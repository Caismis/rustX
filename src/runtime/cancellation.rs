//! RustX-owned cancellation signal.
//!
//! [`CancellationSignal`] is the one runtime-owned cancellation primitive
//! shared by the model plane and the tool plane: model adapter invocations,
//! the compaction summarizer, foreground tool execution, and detached
//! background executions all observe the same underlying signal mechanism.
//! There is exactly one generic cancellation model in the runtime.
//!
//! The signal itself is semantics-free: it does not own an attempt reason,
//! a background lifecycle, or any execution ownership. [`AgentCancellation`]
//! wraps it with the attempt cancellation reason, and the background
//! registry owns its own background cancellation state.
//!
//! [`AgentCancellation`]: crate::agent::cancellation::AgentCancellation

use tokio_util::sync::CancellationToken;

/// Cancellation signal for one operation.
///
/// Cancelling the signal makes every pending [`CancellationSignal::cancelled`]
/// future resolve immediately. Model adapters stop consuming the provider
/// stream, drop the underlying HTTP stream, do not retry, and terminate with
/// `Failed(Cancelled)`; cancellable tool executors settle their external work
/// (for example by terminating an owned process group) and return a
/// normalized cancelled result.
#[derive(Clone, Debug, Default)]
pub struct CancellationSignal {
    token: CancellationToken,
}

impl CancellationSignal {
    /// Creates a new cancellation signal.
    #[must_use]
    pub fn new() -> Self {
        Self {
            token: CancellationToken::new(),
        }
    }

    /// Requests cancellation of the associated operation.
    pub fn cancel(&self) {
        self.token.cancel();
    }

    /// Creates a child signal: cancelling this signal cancels the child, so
    /// one owner can fan out one signal per operation while all of them
    /// terminate together.
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
    use super::CancellationSignal;

    /// A fresh cancellation signal is not cancelled and resolves `cancelled`
    /// only after `cancel` is invoked.
    #[tokio::test]
    async fn cancellation_signal_transitions() {
        let signal = CancellationSignal::new();
        assert!(!signal.is_cancelled());
        signal.cancel();
        assert!(signal.is_cancelled());
        signal.cancelled().await;
    }

    /// Clones share the same underlying signal.
    #[tokio::test]
    async fn cloned_signals_share_cancellation() {
        let first = CancellationSignal::new();
        let second = first.clone();
        first.cancel();
        second.cancelled().await;
        assert!(second.is_cancelled());
    }

    /// `cancelled` resolves even if cancellation arrived before it was
    /// awaited, which is how a stream loop notices an early cancellation.
    #[tokio::test]
    async fn cancelled_after_the_fact_resolves_immediately() {
        let signal = CancellationSignal::new();
        signal.cancel();
        tokio::time::timeout(std::time::Duration::from_secs(1), signal.cancelled())
            .await
            .expect("cancelled must resolve without waiting");
    }

    /// A child signal is cancelled when its parent is cancelled, so one
    /// owner-level signal can govern every operation of an owner.
    #[tokio::test]
    async fn child_signals_follow_parent_cancellation() {
        let parent = CancellationSignal::new();
        let child = parent.child();
        assert!(!child.is_cancelled());
        parent.cancel();
        assert!(child.is_cancelled());
        tokio::time::timeout(std::time::Duration::from_secs(1), child.cancelled())
            .await
            .expect("child must be cancelled with its parent");
    }
}
