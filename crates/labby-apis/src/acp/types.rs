//! ACP domain types — session state machine, events, content blocks, and
//! supporting structs. All public types use the `Acp*` prefix.

use serde::{Deserialize, Serialize};
use serde_json::Value;

// ---------------------------------------------------------------------------
// Session state machine
// ---------------------------------------------------------------------------

/// Eight-state machine for an ACP session lifecycle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AcpSessionState {
    Creating,
    Idle,
    Running,
    WaitingForPermission,
    Completed,
    Cancelled,
    Failed,
    Closed,
}

impl AcpSessionState {
    /// Returns `true` if the transition from `self` to `next` is valid.
    pub fn can_transition_to(&self, next: &Self) -> bool {
        matches!(
            (self, next),
            (Self::Creating, Self::Idle | Self::Failed)
                | (Self::Idle | Self::Completed, Self::Running | Self::Closed)
                | (
                    Self::Running,
                    Self::Completed | Self::Failed | Self::Cancelled | Self::WaitingForPermission
                )
                | (
                    Self::WaitingForPermission,
                    Self::Running | Self::Cancelled | Self::Failed
                )
                | (Self::Cancelled | Self::Failed, Self::Closed)
        )
    }

    /// Returns `true` if this is a terminal state (no further transitions allowed).
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Closed)
    }
}

// ---------------------------------------------------------------------------
// Session summary
// ---------------------------------------------------------------------------

/// Lightweight summary of an ACP session returned by list/get endpoints.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct AcpSessionSummary {
    pub id: String,
    pub provider: String,
    pub title: String,
    pub cwd: String,
    pub state: AcpSessionState,
    pub created_at: String,
    pub updated_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub principal: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_name: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub config_options: Vec<AcpSessionConfigOptionView>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub struct AcpModelOption {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub fixed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub struct AcpSessionConfigOptionView {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_value: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<AcpModelOption>,
}

// ---------------------------------------------------------------------------
// Permission
// ---------------------------------------------------------------------------

/// A single permission choice offered to the user during a permission request.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct AcpPermissionOption {
    pub option_id: String,
    pub name: String,
    pub kind: String,
}

// ---------------------------------------------------------------------------
// Provider health
// ---------------------------------------------------------------------------

/// Health report for a single ACP provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct AcpProviderHealth {
    pub provider: String,
    pub available: bool,
    pub version: Option<String>,
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<AcpModelOption>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_model_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_model_id: Option<String>,
}

// ---------------------------------------------------------------------------
// Content blocks (Phase 1 — exactly 5 variants)
// ---------------------------------------------------------------------------

/// Phase 1 content block variants.
///
/// Deferred to Phase 2: Image, FileTree, WebPreview, Citations.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AcpContentBlock {
    Text {
        text: String,
    },
    Reasoning {
        text: String,
    },
    ToolCall {
        tool_call_id: String,
        name: String,
        input: Value,
        #[serde(skip_serializing_if = "Option::is_none")]
        output: Option<Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        status: Option<String>,
    },
    Code {
        #[serde(skip_serializing_if = "Option::is_none")]
        language: Option<String>,
        code: String,
    },
    Unknown {
        #[serde(rename = "type_tag")]
        type_tag: String,
        raw: Value,
    },
}

// ---------------------------------------------------------------------------
// Discriminated event enum
// ---------------------------------------------------------------------------

/// Discriminated ACP event enum. Each variant is tagged by `kind`.
///
/// Every variant carries the four common envelope fields:
/// `id`, `created_at`, `session_id`, `seq`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AcpEvent {
    MessageChunk {
        id: String,
        created_at: String,
        session_id: String,
        seq: u64,
        #[serde(default)]
        provider: String,
        role: String,
        text: String,
        message_id: String,
    },
    ReasoningChunk {
        id: String,
        created_at: String,
        session_id: String,
        seq: u64,
        #[serde(default)]
        provider: String,
        text: String,
    },
    ToolCallStart {
        id: String,
        created_at: String,
        session_id: String,
        seq: u64,
        #[serde(default)]
        provider: String,
        tool_call_id: String,
        name: String,
        input: Value,
    },
    ToolCallUpdate {
        id: String,
        created_at: String,
        session_id: String,
        seq: u64,
        #[serde(default)]
        provider: String,
        tool_call_id: String,
        output: Value,
        status: String,
    },
    PermissionRequest {
        id: String,
        created_at: String,
        session_id: String,
        seq: u64,
        #[serde(default)]
        provider: String,
        request_id: String,
        action_summary: String,
        options: Vec<AcpPermissionOption>,
    },
    PermissionOutcome {
        id: String,
        created_at: String,
        session_id: String,
        seq: u64,
        #[serde(default)]
        provider: String,
        request_id: String,
        granted: bool,
    },
    UsageUpdate {
        id: String,
        created_at: String,
        session_id: String,
        seq: u64,
        #[serde(default)]
        provider: String,
        raw: Value,
    },
    ContentBlocks {
        id: String,
        created_at: String,
        session_id: String,
        seq: u64,
        #[serde(default)]
        provider: String,
        blocks: Vec<AcpContentBlock>,
    },
    SessionUpdate {
        id: String,
        created_at: String,
        session_id: String,
        seq: u64,
        #[serde(default)]
        provider: String,
        state: AcpSessionState,
    },
    ProviderSwitch {
        id: String,
        created_at: String,
        session_id: String,
        seq: u64,
        from_provider: String,
        to_provider: String,
        continuity_mode: String,
        message: String,
    },
    ProviderInfo {
        id: String,
        created_at: String,
        session_id: String,
        seq: u64,
        provider: String,
        raw: Value,
    },
    Unknown {
        id: String,
        created_at: String,
        session_id: String,
        seq: u64,
        /// The original event kind string (preserved for round-tripping unknown variants).
        /// Named `event_kind` to avoid colliding with the `#[serde(tag = "kind")]` field.
        #[serde(rename = "event_kind")]
        event_kind: String,
        raw: Value,
    },
}

impl AcpEvent {
    /// Returns the sequence number for this event.
    pub fn seq(&self) -> u64 {
        match self {
            Self::MessageChunk { seq, .. }
            | Self::ReasoningChunk { seq, .. }
            | Self::ToolCallStart { seq, .. }
            | Self::ToolCallUpdate { seq, .. }
            | Self::PermissionRequest { seq, .. }
            | Self::PermissionOutcome { seq, .. }
            | Self::UsageUpdate { seq, .. }
            | Self::ContentBlocks { seq, .. }
            | Self::SessionUpdate { seq, .. }
            | Self::ProviderSwitch { seq, .. }
            | Self::ProviderInfo { seq, .. }
            | Self::Unknown { seq, .. } => *seq,
        }
    }

    /// Returns the session ID for this event.
    pub fn session_id(&self) -> &str {
        match self {
            Self::MessageChunk { session_id, .. }
            | Self::ReasoningChunk { session_id, .. }
            | Self::ToolCallStart { session_id, .. }
            | Self::ToolCallUpdate { session_id, .. }
            | Self::PermissionRequest { session_id, .. }
            | Self::PermissionOutcome { session_id, .. }
            | Self::UsageUpdate { session_id, .. }
            | Self::ContentBlocks { session_id, .. }
            | Self::SessionUpdate { session_id, .. }
            | Self::ProviderSwitch { session_id, .. }
            | Self::ProviderInfo { session_id, .. }
            | Self::Unknown { session_id, .. } => session_id,
        }
    }

    /// Returns the provider that owns this event, when the event represents
    /// provider-owned turn output or a provider switch target.
    pub fn provider_id(&self) -> Option<&str> {
        match self {
            Self::MessageChunk { provider, .. }
            | Self::ReasoningChunk { provider, .. }
            | Self::ToolCallStart { provider, .. }
            | Self::ToolCallUpdate { provider, .. }
            | Self::PermissionRequest { provider, .. }
            | Self::PermissionOutcome { provider, .. }
            | Self::UsageUpdate { provider, .. }
            | Self::ContentBlocks { provider, .. }
            | Self::SessionUpdate { provider, .. }
            | Self::ProviderInfo { provider, .. } => {
                if provider.is_empty() {
                    None
                } else {
                    Some(provider)
                }
            }
            Self::ProviderSwitch { to_provider, .. } => Some(to_provider),
            Self::Unknown { .. } => None,
        }
    }
}
