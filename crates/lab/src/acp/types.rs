#![allow(dead_code)]

//! ACP types for the `lab` binary crate.
//!
//! Canonical public types live in `lab_apis::acp::types` under the `Acp*`
//! prefix. This file re-exports them for convenience and keeps the legacy
//! `Bridge*` shapes only as compatibility projections for browser-local legacy
//! consumers that have not finished the typed ACP migration.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[allow(unused_imports)]
pub use lab_apis::acp::{
    AcpContentBlock, AcpError, AcpEvent, AcpPermissionOption, AcpProviderHealth, AcpSessionState,
    AcpSessionSummary, PersistenceError,
};

pub type AcpProviderKind = String;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderHealth {
    pub provider: AcpProviderKind,
    pub ready: bool,
    pub command: String,
    pub args: Vec<String>,
    pub message: String,
    pub models: Vec<lab_apis::acp::types::AcpModelOption>,
    pub default_model_id: Option<String>,
    pub current_model_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BridgeSessionSummary {
    pub id: String,
    pub provider_session_id: String,
    pub provider: AcpProviderKind,
    pub title: String,
    pub cwd: String,
    pub created_at: String,
    pub updated_at: String,
    pub status: String,
    pub agent_name: String,
    pub agent_version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resumable: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BridgePermissionOption {
    pub option_id: String,
    pub name: String,
    pub kind: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BridgeEvent {
    pub id: String,
    pub seq: u64,
    pub session_id: String,
    pub provider: AcpProviderKind,
    pub kind: String,
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_input: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_output: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_content: Option<Vec<Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub locations: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan: Option<Vec<Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permission_options: Option<Vec<BridgePermissionOption>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permission_selection: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_info: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commands: Option<Vec<Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_mode: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config_update: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_stop_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw: Option<Value>,
}

#[must_use]
pub(crate) fn stamp_event_sequence(event: AcpEvent, seq: u64) -> AcpEvent {
    match event {
        AcpEvent::MessageChunk {
            id,
            created_at,
            session_id,
            role,
            text,
            message_id,
            ..
        } => AcpEvent::MessageChunk {
            id,
            created_at,
            session_id,
            seq,
            role,
            text,
            message_id,
        },
        AcpEvent::ReasoningChunk {
            id,
            created_at,
            session_id,
            text,
            ..
        } => AcpEvent::ReasoningChunk {
            id,
            created_at,
            session_id,
            seq,
            text,
        },
        AcpEvent::ToolCallStart {
            id,
            created_at,
            session_id,
            tool_call_id,
            name,
            input,
            ..
        } => AcpEvent::ToolCallStart {
            id,
            created_at,
            session_id,
            seq,
            tool_call_id,
            name,
            input,
        },
        AcpEvent::ToolCallUpdate {
            id,
            created_at,
            session_id,
            tool_call_id,
            output,
            status,
            ..
        } => AcpEvent::ToolCallUpdate {
            id,
            created_at,
            session_id,
            seq,
            tool_call_id,
            output,
            status,
        },
        AcpEvent::PermissionRequest {
            id,
            created_at,
            session_id,
            request_id,
            action_summary,
            options,
            ..
        } => AcpEvent::PermissionRequest {
            id,
            created_at,
            session_id,
            seq,
            request_id,
            action_summary,
            options,
        },
        AcpEvent::PermissionOutcome {
            id,
            created_at,
            session_id,
            request_id,
            granted,
            ..
        } => AcpEvent::PermissionOutcome {
            id,
            created_at,
            session_id,
            seq,
            request_id,
            granted,
        },
        AcpEvent::UsageUpdate {
            id,
            created_at,
            session_id,
            raw,
            ..
        } => AcpEvent::UsageUpdate {
            id,
            created_at,
            session_id,
            seq,
            raw,
        },
        AcpEvent::ContentBlocks {
            id,
            created_at,
            session_id,
            blocks,
            ..
        } => AcpEvent::ContentBlocks {
            id,
            created_at,
            session_id,
            seq,
            blocks,
        },
        AcpEvent::SessionUpdate {
            id,
            created_at,
            session_id,
            state,
            ..
        } => AcpEvent::SessionUpdate {
            id,
            created_at,
            session_id,
            seq,
            state,
        },
        AcpEvent::ProviderInfo {
            id,
            created_at,
            session_id,
            provider,
            raw,
            ..
        } => AcpEvent::ProviderInfo {
            id,
            created_at,
            session_id,
            seq,
            provider,
            raw,
        },
        AcpEvent::Unknown {
            id,
            created_at,
            session_id,
            event_kind,
            raw,
            ..
        } => AcpEvent::Unknown {
            id,
            created_at,
            session_id,
            seq,
            event_kind,
            raw,
        },
    }
}

#[must_use]
pub(crate) fn event_created_at(event: &AcpEvent) -> &str {
    match event {
        AcpEvent::MessageChunk { created_at, .. }
        | AcpEvent::ReasoningChunk { created_at, .. }
        | AcpEvent::ToolCallStart { created_at, .. }
        | AcpEvent::ToolCallUpdate { created_at, .. }
        | AcpEvent::PermissionRequest { created_at, .. }
        | AcpEvent::PermissionOutcome { created_at, .. }
        | AcpEvent::UsageUpdate { created_at, .. }
        | AcpEvent::ContentBlocks { created_at, .. }
        | AcpEvent::SessionUpdate { created_at, .. }
        | AcpEvent::ProviderInfo { created_at, .. }
        | AcpEvent::Unknown { created_at, .. } => created_at,
    }
}

#[must_use]
pub(crate) fn session_title_from_event(event: &AcpEvent) -> Option<String> {
    match event {
        AcpEvent::ProviderInfo { raw, .. } => raw
            .get("type")
            .and_then(Value::as_str)
            .filter(|kind| *kind == "session_info")
            .and_then(|_| {
                raw.get("session_info")
                    .and_then(|value| value.get("title"))
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned)
                    .or_else(|| {
                        raw.get("title")
                            .and_then(Value::as_str)
                            .map(ToOwned::to_owned)
                    })
            }),
        _ => None,
    }
}

#[derive(Debug, Clone)]
pub struct StartSessionInput {
    pub provider: Option<String>,
    pub cwd: String,
    pub title: Option<String>,
    pub principal: Option<String>,
    pub model_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct StartSessionResult {
    pub provider_session_id: String,
    pub agent_name: String,
    pub agent_version: String,
    pub model_id: Option<String>,
    pub model_name: Option<String>,
    pub models: Vec<lab_apis::acp::types::AcpModelOption>,
    pub config_options: Vec<lab_apis::acp::types::AcpSessionConfigOptionView>,
}
