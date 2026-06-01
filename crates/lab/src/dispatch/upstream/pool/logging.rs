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
    tracing::debug!(
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
