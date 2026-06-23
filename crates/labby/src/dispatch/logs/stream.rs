//! Broadcast fanout hub for live log streaming.

use tokio::sync::broadcast;

use super::types::{LogEvent, LogStreamReceiver, StreamSubscription};

pub struct StreamHub {
    sender: broadcast::Sender<LogEvent>,
}

impl StreamHub {
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(capacity.max(1));
        Self { sender }
    }

    /// Publish an event to all live subscribers. Non-blocking; if there are
    /// no subscribers, `send` returns `Err(SendError)` which we swallow.
    pub fn publish(&self, event: LogEvent) {
        drop(self.sender.send(event));
    }

    /// Create a new receiver. Filtering is applied on the receiver side so
    /// the shared broadcast channel carries every event — a slow subscriber
    /// cannot starve others.
    #[must_use]
    pub fn subscribe(&self, filter: StreamSubscription) -> LogStreamReceiver {
        LogStreamReceiver::new(self.sender.subscribe(), filter)
    }
}
