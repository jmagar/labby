//! `SessionHandle` and `SessionCommand` — bounded channel abstraction for a
//! running provider session.
//!
//! The `prompt_tx` field is a **bounded** `mpsc::Sender` (capacity 64).
//! Back-pressure is applied to callers when the provider is busy.

use thiserror::Error;
use tokio::sync::mpsc;

/// Command sent to a running provider session.
#[derive(Debug)]
pub enum SessionCommand {
    /// Send a prompt to the provider.
    Prompt { text: String },
    /// Request cancellation of the current operation.
    Cancel,
}

/// Handle to a running provider session.
///
/// Cloning is intentionally not derived — callers should share via `Arc<SessionHandle>`
/// if fan-out is needed. The bounded sender can be cloned explicitly by calling
/// `prompt_tx.clone()` when required.
pub struct SessionHandle {
    /// Name of the provider backing this session (e.g. `"codex"`).
    pub provider: String,
    /// Bounded channel (capacity 64). Back-pressure on caller if provider is busy.
    pub prompt_tx: mpsc::Sender<SessionCommand>,
}

impl SessionHandle {
    /// Send a prompt to the provider session.
    ///
    /// Returns `Err(SessionError::ChannelClosed)` if the session has ended.
    /// Returns `Err(SessionError::BufferFull)` if the channel is at capacity
    /// and the send would block (use `try_send` semantics are not exposed here;
    /// this call awaits until there is space or the channel closes).
    pub async fn send_prompt(&self, text: String) -> Result<(), SessionError> {
        self.prompt_tx
            .send(SessionCommand::Prompt { text })
            .await
            .map_err(|_| SessionError::ChannelClosed)
    }

    /// Request cancellation of the current operation.
    pub async fn cancel(&self) -> Result<(), SessionError> {
        self.prompt_tx
            .send(SessionCommand::Cancel)
            .await
            .map_err(|_| SessionError::ChannelClosed)
    }
}

/// Errors that can occur when communicating with a provider session.
#[derive(Debug, Error)]
pub enum SessionError {
    /// The provider session has ended and the channel is closed.
    #[error("session channel closed")]
    ChannelClosed,
    /// The prompt buffer is full (provider is busy).
    #[error("session backpressure: prompt buffer full")]
    BufferFull,
}
