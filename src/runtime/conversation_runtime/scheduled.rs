//! Conversation-owned scheduled execution: no synthetic `Attempt` or `ToolCall`.
use super::{Arc, ConversationRuntime};

impl ConversationRuntime {
    /// A wake handle carries no runtime ownership or execution authority.
    pub(crate) fn scheduled_wake(&self) -> Arc<tokio::sync::Notify> {
        self.inner.mailbox.wake()
    }
}
