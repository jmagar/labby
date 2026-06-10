//! Upstream request-logging helpers.
//!
//! `UpstreamRequestLog` carries the structured fields documented in
//! `docs/dev/OBSERVABILITY.md` for an upstream tool/resource/prompt RPC, and the
//! `log_upstream_request_{start,finish,error}` helpers emit the canonical events.
//!
//! These items are `pub(super)` because the capability modules
//! (`tools_call`, `resources_read`, `prompts_get`) construct `UpstreamRequestLog`
//! and call the log helpers across the module boundary.

use super::super::types::UpstreamCapability;

pub(super) fn is_capability_unsupported(error: &rmcp::ServiceError) -> bool {
    // Prefer the structured JSON-RPC code: a `-32601 Method not found` reply
    // means the upstream simply doesn't implement that capability.
    if let rmcp::ServiceError::McpError(data) = error
        && data.code == rmcp::model::ErrorCode::METHOD_NOT_FOUND
    {
        return true;
    }
    // Fallback to message matching for transports/servers that surface the
    // same condition without a clean structured code.
    let msg = error.to_string();
    msg.contains("Method not found")
        || msg.contains("method_not_found")
        || msg.contains("-32601")
        || msg.contains("Not implemented")
}

pub(super) fn capability_name(capability: UpstreamCapability) -> &'static str {
    match capability {
        UpstreamCapability::Tools => "tools",
        UpstreamCapability::Prompts => "prompts",
        UpstreamCapability::Resources => "resources",
    }
}

#[derive(Clone, Copy)]
pub(super) struct UpstreamRequestLog<'a> {
    pub(super) upstream: &'a str,
    pub(super) capability: &'static str,
    pub(super) operation: &'static str,
    pub(super) subject_scoped: bool,
    pub(super) transport: Option<&'static str>,
    pub(super) item_kind: Option<&'static str>,
    pub(super) item: Option<&'a str>,
}

impl<'a> UpstreamRequestLog<'a> {
    pub(super) fn tool(upstream: &'a str, tool: &'a str, subject_scoped: bool) -> Self {
        Self {
            upstream,
            capability: "tools",
            operation: "tool.call",
            subject_scoped,
            transport: None,
            item_kind: Some("tool"),
            item: Some(tool),
        }
    }

    pub(super) fn resource(upstream: &'a str, resource_uri: &'a str, subject_scoped: bool) -> Self {
        Self {
            upstream,
            capability: "resources",
            operation: "resource.read",
            subject_scoped,
            transport: None,
            item_kind: Some("resource_uri"),
            item: Some(resource_uri),
        }
    }

    pub(super) fn prompt(upstream: &'a str, prompt: &'a str, subject_scoped: bool) -> Self {
        Self {
            upstream,
            capability: "prompts",
            operation: "prompt.get",
            subject_scoped,
            transport: None,
            item_kind: Some("prompt"),
            item: Some(prompt),
        }
    }

    pub(super) fn with_transport(mut self, transport: &'static str) -> Self {
        self.transport = Some(transport);
        self
    }
}

pub(super) fn log_upstream_request_start(event: UpstreamRequestLog<'_>) {
    tracing::info!(
        surface = "dispatch",
        service = "upstream.pool",
        action = "upstream.request",
        event = "start",
        upstream = %event.upstream,
        capability = event.capability,
        operation = event.operation,
        subject_scoped = event.subject_scoped,
        transport = event.transport,
        item_kind = event.item_kind,
        item = event.item,
        "upstream.request.start"
    );
}

pub(super) fn log_upstream_request_finish(
    event: UpstreamRequestLog<'_>,
    elapsed_ms: u128,
    response_bytes: Option<usize>,
) {
    tracing::info!(
        surface = "dispatch",
        service = "upstream.pool",
        action = "upstream.request",
        event = "finish",
        upstream = %event.upstream,
        capability = event.capability,
        operation = event.operation,
        subject_scoped = event.subject_scoped,
        transport = event.transport,
        item_kind = event.item_kind,
        item = event.item,
        elapsed_ms,
        response_bytes,
        "upstream.request.finish"
    );
}

pub(super) fn log_upstream_request_error(
    event: UpstreamRequestLog<'_>,
    elapsed_ms: u128,
    kind: &'static str,
    error: Option<&dyn std::fmt::Display>,
    response_bytes: Option<usize>,
    max_bytes: Option<usize>,
) {
    tracing::warn!(
        surface = "dispatch",
        service = "upstream.pool",
        action = "upstream.request",
        event = "error",
        upstream = %event.upstream,
        capability = event.capability,
        operation = event.operation,
        subject_scoped = event.subject_scoped,
        transport = event.transport,
        item_kind = event.item_kind,
        item = event.item,
        elapsed_ms,
        kind,
        error = error.map(tracing::field::display),
        response_bytes,
        max_bytes,
        "upstream.request.error"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use tracing_subscriber::layer::SubscriberExt;

    /// O-L3: `request_id` propagates through the OAuth subject-scoped reconnect
    /// fan-out.  Every log event emitted during a subject-scoped call — including
    /// the `upstream_connect_error` emitted when `acquire_or_connect_subject` fails
    /// — must carry the `request_id` inherited from the outer dispatch span.
    ///
    /// We simulate the fan-out by entering a dispatch span with a `request_id`
    /// field and then calling `log_upstream_request_start` followed by
    /// `log_upstream_request_error` with `kind = "upstream_connect_error"`, which
    /// is exactly the sequence emitted by `subject_scoped_call_tool` when the OAuth
    /// connect step fails.  If `request_id` is present in all three events the
    /// span-context threading is correct.
    #[test]
    fn request_id_propagates_through_subject_scoped_reconnect_fan_out() {
        let _tracing_lock = crate::test_support::TRACING_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let buf = crate::test_support::SharedBuf::default();
        let subscriber = tracing_subscriber::registry()
            .with(tracing_subscriber::EnvFilter::new("labby=info"))
            .with(
                tracing_subscriber::fmt::layer()
                    .json()
                    .with_current_span(true)
                    .with_writer(buf.clone())
                    .with_ansi(false)
                    .without_time(),
            );
        let _guard = tracing::subscriber::set_default(subscriber);

        // Simulate the outer dispatch span that carries the request_id — this is
        // created by the MCP/HTTP surface before calling into the pool.
        let dispatch_span = tracing::info_span!(
            "dispatch",
            surface = "mcp",
            service = "gateway",
            action = "tools/call",
            request_id = "req-oauth-fan-out-456"
        );
        let _entered = dispatch_span.enter();

        // First hop: start event (subject_scoped = true, transport = "http").
        let event =
            UpstreamRequestLog::tool("oauth-upstream", "my_tool", true).with_transport("http");
        log_upstream_request_start(event);

        // Second hop: the connect error path — simulates `acquire_or_connect_subject`
        // failing and the caller emitting "upstream_connect_error".
        log_upstream_request_error(
            event,
            5,
            "upstream_connect_error",
            Some(&"OAuth token exchange failed"),
            None,
            None,
        );

        // Third hop: a normal error on a different subject (different fan-out branch).
        let event2 =
            UpstreamRequestLog::tool("oauth-upstream", "my_tool", true).with_transport("http");
        log_upstream_request_error(event2, 12, "timeout", None, None, None);

        drop(_entered);
        drop(_guard);

        let logs = crate::test_support::captured_logs(&buf);

        // Every log line must carry request_id from the outer span.
        let lines: Vec<&str> = logs.lines().filter(|l| !l.trim().is_empty()).collect();
        assert!(
            !lines.is_empty(),
            "expected at least one log line, got none"
        );
        for line in &lines {
            assert!(
                line.contains("\"request_id\":\"req-oauth-fan-out-456\""),
                "request_id missing from log line:\n{line}"
            );
            assert!(
                line.contains("\"subject_scoped\":true"),
                "subject_scoped field missing from log line:\n{line}"
            );
        }

        // The connect error hop must carry "upstream_connect_error" kind.
        assert!(
            logs.contains("\"kind\":\"upstream_connect_error\""),
            "upstream_connect_error kind not found in logs:\n{logs}"
        );
        // The timeout hop must carry "timeout" kind.
        assert!(
            logs.contains("\"kind\":\"timeout\""),
            "timeout kind not found in logs:\n{logs}"
        );
    }

    #[test]
    fn upstream_request_log_helpers_emit_documented_fields_and_inherit_request_id() {
        let _tracing_lock = crate::test_support::TRACING_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let buf = crate::test_support::SharedBuf::default();
        let subscriber = tracing_subscriber::registry()
            .with(tracing_subscriber::EnvFilter::new("labby=debug"))
            .with(
                tracing_subscriber::fmt::layer()
                    .json()
                    .with_current_span(true)
                    .with_writer(buf.clone())
                    .with_ansi(false)
                    .without_time(),
            );
        let _guard = tracing::subscriber::set_default(subscriber);

        let span = tracing::info_span!(
            "dispatch",
            surface = "api",
            service = "gateway",
            action = "call_tool",
            request_id = "req-123"
        );
        let _entered = span.enter();

        let event = UpstreamRequestLog::tool("github", "search_repos", false);
        log_upstream_request_start(event);
        log_upstream_request_finish(event, 7, Some(128));
        log_upstream_request_error(event, 9, "upstream_error", Some(&"boom"), None, None);

        drop(_entered);
        drop(_guard);

        let logs = crate::test_support::captured_logs(&buf);
        for expected in [
            "\"request_id\":\"req-123\"",
            "\"surface\":\"dispatch\"",
            "\"service\":\"upstream.pool\"",
            "\"action\":\"upstream.request\"",
            "\"upstream\":\"github\"",
            "\"capability\":\"tools\"",
            "\"operation\":\"tool.call\"",
            "\"event\":\"start\"",
            "\"event\":\"finish\"",
            "\"event\":\"error\"",
            "\"elapsed_ms\":\"7\"",
            "\"elapsed_ms\":\"9\"",
            "\"kind\":\"upstream_error\"",
        ] {
            assert!(
                logs.contains(expected),
                "missing upstream request log field `{expected}` in:\n{logs}"
            );
        }
    }
}
