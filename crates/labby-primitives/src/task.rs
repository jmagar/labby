//! Agent Task contracts. These are not Depot ingestion/artifact jobs.

use crate::access::{OwnerScope, PrincipalId, ProjectId};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TaskState {
    Created,
    Queued,
    Running,
    Cancelling,
    Succeeded,
    Failed,
    Cancelled,
    Expired,
}

impl TaskState {
    pub const fn terminal(self) -> bool {
        matches!(
            self,
            Self::Succeeded | Self::Failed | Self::Cancelled | Self::Expired
        )
    }
    pub const fn permits(self, next: Self) -> bool {
        matches!(
            (self, next),
            (
                Self::Created | Self::Failed | Self::Cancelled | Self::Expired,
                Self::Queued
            ) | (Self::Queued, Self::Running)
                | (
                    Self::Created | Self::Queued | Self::Running,
                    Self::Cancelling
                )
                | (Self::Running, Self::Succeeded | Self::Failed)
                | (Self::Cancelling, Self::Cancelled | Self::Failed)
                | (
                    Self::Queued | Self::Running | Self::Cancelling,
                    Self::Expired
                )
        )
    }
    pub const fn wire(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Cancelling => "cancelling",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Expired => "expired",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TaskIntent {
    pub id: String,
    pub idempotency_key: String,
    pub owner: OwnerScope,
    pub project: Option<ProjectId>,
    pub creator: PrincipalId,
    pub agent_id: String,
    pub agent_version: u64,
    pub agent_revision_digest: String,
    pub input_digest: String,
    pub catalog_generation: String,
    pub authority_fingerprint: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TaskAttemptLease {
    pub attempt: u32,
    pub fencing_token: String,
    pub expires_at: i64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OutputVisibility {
    CreatorOnly,
    OwnerMembers,
    ExplicitReaders,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TaskSettlement {
    pub state: TaskState,
    pub output_digest: Option<String>,
    pub error_code: Option<String>,
    pub settled_at: i64,
}

pub fn validate_intent(intent: &TaskIntent) -> bool {
    intent.agent_version > 0
        && [
            &intent.id,
            &intent.idempotency_key,
            &intent.agent_id,
            &intent.catalog_generation,
            &intent.authority_fingerprint,
        ]
        .iter()
        .all(|v| valid(v))
        && [&intent.agent_revision_digest, &intent.input_digest]
            .iter()
            .all(|v| digest(v))
}
fn valid(v: &str) -> bool {
    !v.is_empty() && v.len() <= 256 && v == v.trim() && !v.chars().any(char::is_control)
}
fn digest(v: &str) -> bool {
    v.strip_prefix("sha256:").is_some_and(|h| {
        h.len() == 64
            && h.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lifecycle_is_closed_and_terminal_settlement_is_final() {
        assert!(TaskState::Created.permits(TaskState::Queued));
        assert!(TaskState::Running.permits(TaskState::Succeeded));
        assert!(!TaskState::Succeeded.permits(TaskState::Running));
        assert!(TaskState::Succeeded.terminal());
        assert!(!TaskState::Running.terminal());
    }
}
