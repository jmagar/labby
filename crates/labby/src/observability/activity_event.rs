//! Typed activity/log event builder.
//!
//! New activity-producing callsites should use this builder instead of spelling
//! taxonomy fields as string literals. The ingest boundary still accepts
//! `RawLogEvent`, so this module converts typed values into the existing wire
//! shape without changing the log store schema.

#![allow(dead_code)]

use serde::Serialize;

use crate::dispatch::logs::types::{LogLevel, LogSystem, RawLogEvent, Subsystem, Surface};
use crate::observability::activity::ActorKey;

#[derive(Clone, Debug, PartialEq)]
pub struct ActivityEvent {
    level: LogLevel,
    subsystem: Subsystem,
    surface: Surface,
    action: String,
    message: String,
    request_id: Option<String>,
    session_id: Option<String>,
    correlation_id: Option<String>,
    trace_id: Option<String>,
    span_id: Option<String>,
    instance: Option<String>,
    auth_flow: Option<String>,
    outcome_kind: Option<String>,
    fields_json: serde_json::Value,
    source_kind: Option<String>,
    source_node_id: Option<String>,
    source_device_id: Option<String>,
    actor_key: Option<String>,
    ingest_path: Option<String>,
    upstream_event_id: Option<String>,
}

impl ActivityEvent {
    #[must_use]
    pub fn builder(subsystem: Subsystem, action: impl Into<String>) -> ActivityEventBuilder {
        ActivityEventBuilder::new(subsystem, action)
    }

    #[must_use]
    pub const fn level(&self) -> LogLevel {
        self.level
    }

    #[must_use]
    pub const fn subsystem(&self) -> Subsystem {
        self.subsystem
    }

    #[must_use]
    pub const fn surface(&self) -> Surface {
        self.surface
    }

    #[must_use]
    pub fn action(&self) -> &str {
        &self.action
    }

    #[must_use]
    pub fn to_raw_log_event(&self) -> RawLogEvent {
        RawLogEvent {
            ts: None,
            level: Some(self.level.as_str().to_string()),
            subsystem: Some(self.subsystem.as_str().to_string()),
            surface: Some(self.surface.as_str().to_string()),
            action: Some(self.action.clone()),
            message: self.message.clone(),
            request_id: self.request_id.clone(),
            session_id: self.session_id.clone(),
            correlation_id: self.correlation_id.clone(),
            trace_id: self.trace_id.clone(),
            span_id: self.span_id.clone(),
            instance: self.instance.clone(),
            auth_flow: self.auth_flow.clone(),
            outcome_kind: self.outcome_kind.clone(),
            fields_json: self.fields_json.clone(),
            source_kind: self.source_kind.clone(),
            source_node_id: self.source_node_id.clone(),
            source_device_id: self.source_device_id.clone(),
            actor_key: self.actor_key.clone(),
            ingest_path: self.ingest_path.clone(),
            upstream_event_id: self.upstream_event_id.clone(),
        }
    }

    pub fn try_ingest(&self, system: &LogSystem) -> Result<(), crate::dispatch::error::ToolError> {
        system.try_ingest(self.to_raw_log_event())
    }
}

#[derive(Clone, Debug)]
pub struct ActivityEventBuilder {
    event: ActivityEvent,
}

impl ActivityEventBuilder {
    #[must_use]
    pub fn new(subsystem: Subsystem, action: impl Into<String>) -> Self {
        let action = action.into();
        Self {
            event: ActivityEvent {
                level: LogLevel::Info,
                subsystem,
                surface: Surface::CoreRuntime,
                message: action.clone(),
                action,
                request_id: None,
                session_id: None,
                correlation_id: None,
                trace_id: None,
                span_id: None,
                instance: None,
                auth_flow: None,
                outcome_kind: None,
                fields_json: serde_json::json!({}),
                source_kind: None,
                source_node_id: None,
                source_device_id: None,
                actor_key: None,
                ingest_path: Some("activity_builder".to_string()),
                upstream_event_id: None,
            },
        }
    }

    #[must_use]
    pub fn level(mut self, level: LogLevel) -> Self {
        self.event.level = level;
        self
    }

    #[must_use]
    pub fn surface(mut self, surface: Surface) -> Self {
        self.event.surface = surface;
        self
    }

    #[must_use]
    pub fn message(mut self, message: impl Into<String>) -> Self {
        self.event.message = message.into();
        self
    }

    #[must_use]
    pub fn request_id(mut self, request_id: impl Into<String>) -> Self {
        self.event.request_id = Some(request_id.into());
        self
    }

    #[must_use]
    pub fn session_id(mut self, session_id: impl Into<String>) -> Self {
        self.event.session_id = Some(session_id.into());
        self
    }

    #[must_use]
    pub fn correlation_id(mut self, correlation_id: impl Into<String>) -> Self {
        self.event.correlation_id = Some(correlation_id.into());
        self
    }

    #[must_use]
    pub fn trace_id(mut self, trace_id: impl Into<String>) -> Self {
        self.event.trace_id = Some(trace_id.into());
        self
    }

    #[must_use]
    pub fn span_id(mut self, span_id: impl Into<String>) -> Self {
        self.event.span_id = Some(span_id.into());
        self
    }

    #[must_use]
    pub fn instance(mut self, instance: impl Into<String>) -> Self {
        self.event.instance = Some(instance.into());
        self
    }

    #[must_use]
    pub fn auth_flow(mut self, auth_flow: impl Into<String>) -> Self {
        self.event.auth_flow = Some(auth_flow.into());
        self
    }

    #[must_use]
    pub fn outcome_kind(mut self, outcome_kind: impl Into<String>) -> Self {
        self.event.outcome_kind = Some(outcome_kind.into());
        self
    }

    #[must_use]
    pub fn source_kind(mut self, source_kind: impl Into<String>) -> Self {
        self.event.source_kind = Some(source_kind.into());
        self
    }

    #[must_use]
    pub fn source_node_id(mut self, source_node_id: impl Into<String>) -> Self {
        self.event.source_node_id = Some(source_node_id.into());
        self
    }

    #[must_use]
    pub fn source_device_id(mut self, source_device_id: impl Into<String>) -> Self {
        self.event.source_device_id = Some(source_device_id.into());
        self
    }

    #[must_use]
    pub fn actor_key(mut self, actor_key: &ActorKey) -> Self {
        self.event.actor_key = Some(actor_key.as_str().to_string());
        self
    }

    #[must_use]
    pub fn ingest_path(mut self, ingest_path: impl Into<String>) -> Self {
        self.event.ingest_path = Some(ingest_path.into());
        self
    }

    #[must_use]
    pub fn upstream_event_id(mut self, upstream_event_id: impl Into<String>) -> Self {
        self.event.upstream_event_id = Some(upstream_event_id.into());
        self
    }

    #[must_use]
    pub fn field(mut self, key: impl Into<String>, value: impl Serialize) -> Self {
        let value = serde_json::to_value(value).unwrap_or(serde_json::Value::Null);
        if !self.event.fields_json.is_object() {
            self.event.fields_json = serde_json::json!({});
        }
        if let Some(fields) = self.event.fields_json.as_object_mut() {
            fields.insert(key.into(), value);
        }
        self
    }

    #[must_use]
    pub fn fields_json(mut self, fields_json: serde_json::Value) -> Self {
        self.event.fields_json = fields_json;
        self
    }

    #[must_use]
    pub fn build(self) -> ActivityEvent {
        self.event
    }

    #[must_use]
    pub fn build_raw_log_event(self) -> RawLogEvent {
        self.build().to_raw_log_event()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::observability::activity::ActorKeyDeriver;

    #[test]
    fn builder_converts_typed_taxonomy_to_raw_event_fields() {
        let raw = ActivityEvent::builder(Subsystem::Gateway, "session.bind")
            .surface(Surface::Web)
            .level(LogLevel::Warn)
            .message("session binding failed")
            .request_id("req-1")
            .build_raw_log_event();

        assert_eq!(raw.subsystem.as_deref(), Some("gateway"));
        assert_eq!(raw.surface.as_deref(), Some("web"));
        assert_eq!(raw.level.as_deref(), Some("warn"));
        assert_eq!(raw.action.as_deref(), Some("session.bind"));
        assert_eq!(raw.message, "session binding failed");
        assert_eq!(raw.request_id.as_deref(), Some("req-1"));
    }

    #[test]
    fn builder_defaults_message_to_action_and_core_runtime_surface() {
        let event = ActivityEvent::builder(Subsystem::CoreRuntime, "runtime.started").build();
        let raw = event.to_raw_log_event();

        assert_eq!(event.subsystem(), Subsystem::CoreRuntime);
        assert_eq!(event.surface(), Surface::CoreRuntime);
        assert_eq!(event.action(), "runtime.started");
        assert_eq!(raw.message, "runtime.started");
        assert_eq!(raw.ingest_path.as_deref(), Some("activity_builder"));
    }

    #[test]
    fn builder_carries_activity_fields_without_raw_subject() {
        let deriver = ActorKeyDeriver::from_secret("installation-secret").unwrap();
        let actor_key = deriver.derive_subject("alice@example.com").unwrap();

        let raw = ActivityEvent::builder(Subsystem::AuthWebui, "auth.logout")
            .surface(Surface::Api)
            .actor_key(&actor_key)
            .session_id("sess-1")
            .correlation_id("corr-1")
            .outcome_kind("ok")
            .field("elapsed_ms", 25_u64)
            .field("mine_only", true)
            .build_raw_log_event();

        assert_eq!(raw.actor_key.as_deref(), Some(actor_key.as_str()));
        assert_eq!(raw.session_id.as_deref(), Some("sess-1"));
        assert_eq!(raw.correlation_id.as_deref(), Some("corr-1"));
        assert_eq!(raw.outcome_kind.as_deref(), Some("ok"));
        assert_eq!(raw.fields_json["elapsed_ms"], serde_json::json!(25));
        assert_eq!(raw.fields_json["mine_only"], serde_json::json!(true));
        assert!(!raw.fields_json.to_string().contains("alice@example.com"));
    }
}
