//! Attempt-level cancellation for the agent loop.
//!
//! [`AgentCancellation`] is the deterministic cancellation trigger of one
//! attempt. It reuses the existing rustX-owned cancellation mechanism
//! ([`ModelCancellation`] around a tokio-util cancellation token): the agent
//! signal carries the cancellation reason the terminal event must report,
//! and every model invocation of the attempt receives a child signal so one
//! attempt-level cancel terminates the in-flight provider request through
//! the existing adapter contract.
//!
//! A running tool is not force-aborted: the tool interface exposes no
//! cancellation handle in M3. The loop observes cancellation at its check
//! points (before each model event, between tool calls, and after the tool
//! batch) and refuses all further execution progress afterwards, which keeps
//! cancellation deterministic at the runtime boundary.

use crate::model::ModelCancellation;
use crate::runtime::types::CancellationReason;

/// The cancellation signal of one agent attempt.
///
/// The handle is cheap to clone and all clones share one underlying signal.
/// Cancelling the signal makes every pending
/// [`AgentCancellation::cancelled`] future and every model-invocation child
/// signal resolve immediately.
#[derive(Clone, Debug)]
pub struct AgentCancellation {
    signal: ModelCancellation,
    reason: CancellationReason,
}

impl AgentCancellation {
    /// Creates a new attempt cancellation signal reporting `reason` when
    /// the attempt terminates by cancellation.
    #[must_use]
    pub fn new(reason: CancellationReason) -> Self {
        Self {
            signal: ModelCancellation::new(),
            reason,
        }
    }

    /// The cancellation reason reported by the terminal cancellation event.
    #[must_use]
    pub fn reason(&self) -> CancellationReason {
        self.reason
    }

    /// Requests cancellation of the attempt.
    pub fn cancel(&self) {
        self.signal.cancel();
    }

    /// Whether cancellation has been requested.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.signal.is_cancelled()
    }

    /// A future that resolves when cancellation is requested.
    pub async fn cancelled(&self) {
        self.signal.cancelled().await;
    }

    /// A model-invocation signal cancelled together with this attempt signal.
    #[must_use]
    pub fn model_cancellation(&self) -> ModelCancellation {
        self.signal.child()
    }
}

#[cfg(test)]
mod tests {
    use super::AgentCancellation;
    use crate::runtime::types::CancellationReason;

    /// A fresh signal is not cancelled and resolves only after `cancel`.
    #[tokio::test]
    async fn cancellation_signal_transitions() {
        let signal = AgentCancellation::new(CancellationReason::UserRequested);
        assert!(!signal.is_cancelled());
        assert_eq!(signal.reason(), CancellationReason::UserRequested);
        signal.cancel();
        assert!(signal.is_cancelled());
        signal.cancelled().await;
    }

    /// The model-invocation child signal follows the attempt signal.
    #[tokio::test]
    async fn model_invocation_follows_attempt_cancellation() {
        let signal = AgentCancellation::new(CancellationReason::ParentCancelled);
        let invocation = signal.model_cancellation();
        assert!(!invocation.is_cancelled());
        signal.cancel();
        assert!(invocation.is_cancelled());
        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            invocation.cancelled(),
        )
        .await
        .expect("invocation must be cancelled with the attempt");
    }
}
