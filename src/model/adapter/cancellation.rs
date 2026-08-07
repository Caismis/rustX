//! RustX-owned cancellation for model invocations.
//!
//! The adapter interface exposes [`ModelCancellation`] instead of a
//! provider-specific abort handle. It is a small wrapper around a
//! `tokio_util` cancellation token so adapter stream loops can select
//! between the next provider stream item and the cancellation signal.

use tokio_util::sync::CancellationToken;

/// Cancellation signal for one model invocation.
///
/// Cancelling the token makes every pending [`ModelCancellation::cancelled`]
/// future resolve immediately. Adapters stop consuming the provider stream,
/// drop the underlying HTTP stream, do not retry, and terminate with
/// `Failed(Cancelled)`.
#[derive(Clone, Debug, Default)]
pub struct ModelCancellation {
    token: CancellationToken,
}

impl ModelCancellation {
    /// Creates a new cancellation signal.
    #[must_use]
    pub fn new() -> Self {
        Self {
            token: CancellationToken::new(),
        }
    }

    /// Requests cancellation of the associated invocation.
    pub fn cancel(&self) {
        self.token.cancel();
    }

    /// Creates a child signal: cancelling this signal cancels the child, so
    /// an attempt-level signal can fan out one invocation signal per model
    /// request while all of them terminate together.
    #[must_use]
    pub fn child(&self) -> Self {
        Self {
            token: self.token.child_token(),
        }
    }

    /// Whether cancellation has been requested.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.token.is_cancelled()
    }

    /// A future that resolves when cancellation is requested.
    pub async fn cancelled(&self) {
        self.token.cancelled().await;
    }
}

#[cfg(test)]
mod tests {
    use super::ModelCancellation;

    /// A fresh cancellation signal is not cancelled and resolves `cancelled`
    /// only after `cancel` is invoked.
    #[tokio::test]
    async fn cancellation_signal_transitions() {
        let signal = ModelCancellation::new();
        assert!(!signal.is_cancelled());
        signal.cancel();
        assert!(signal.is_cancelled());
        signal.cancelled().await;
    }

    /// Clones share the same underlying signal.
    #[tokio::test]
    async fn cloned_signals_share_cancellation() {
        let first = ModelCancellation::new();
        let second = first.clone();
        first.cancel();
        second.cancelled().await;
        assert!(second.is_cancelled());
    }

    /// `cancelled` resolves even if cancellation arrived before it was
    /// awaited, which is how a stream loop notices an early cancellation.
    #[tokio::test]
    async fn cancelled_after_the_fact_resolves_immediately() {
        let signal = ModelCancellation::new();
        signal.cancel();
        tokio::time::timeout(std::time::Duration::from_secs(1), signal.cancelled())
            .await
            .expect("cancelled must resolve without waiting");
    }

    /// A child signal is cancelled when its parent is cancelled, so one
    /// attempt-level signal governs every model invocation of the attempt.
    #[tokio::test]
    async fn child_signals_follow_parent_cancellation() {
        let parent = ModelCancellation::new();
        let child = parent.child();
        assert!(!child.is_cancelled());
        parent.cancel();
        assert!(child.is_cancelled());
        tokio::time::timeout(std::time::Duration::from_secs(1), child.cancelled())
            .await
            .expect("child must be cancelled with its parent");
    }
}
