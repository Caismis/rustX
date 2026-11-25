//! The shared physical cancellation primitive.
//!
//! A `ProcessSignal` is one owned operation's cancellation token plus its
//! cancellation/commit arbitration gate. Cancelling the signal makes every
//! pending [`ProcessSignal::cancelled`] future resolve immediately; the
//! physical process driver terminates the owned process group. Semantic
//! causes and compaction authorization live in the host runtime's wrapper —
//! the physical package owns only the signal mechanics.

use tokio_util::sync::CancellationToken;

/// Cancellation signal for one owned physical operation.
///
/// Clones share the token; [`ProcessSignal::child`] fans out one token per
/// operation while the ancestor's cancellation still cancels all of them.
/// The commit gate linearizes cancellation against a caller's
/// cancellation-sensitive commit check.
#[derive(Clone, Debug, Default)]
pub struct ProcessSignal {
    token: CancellationToken,
    commit_gate: std::sync::Arc<std::sync::Mutex<()>>,
}

impl ProcessSignal {
    /// Creates a new cancellation signal.
    #[must_use]
    pub fn new() -> Self {
        Self {
            token: CancellationToken::new(),
            commit_gate: std::sync::Arc::default(),
        }
    }

    /// Requests cancellation of the associated operation.
    pub fn cancel(&self) {
        self.cancel_with(|| ());
    }

    /// Publishes the owner's cause and signals under the same commit gate.
    /// No observer can accept cancellation before a concurrently granted commit.
    pub fn cancel_with<T>(&self, publish_cause: impl FnOnce() -> T) -> T {
        let _gate = self.commit_gate.lock().expect("cancellation commit gate");
        let result = publish_cause();
        self.token.cancel();
        result
    }

    /// A short grant shares the ancestor cancellation boundary. The closure
    /// must not await or recursively use this signal.
    pub fn with_active<T>(&self, grant: impl FnOnce() -> T) -> Option<T> {
        let _gate = self.commit_gate.lock().expect("cancellation commit gate");
        (!self.token.is_cancelled()).then(grant)
    }

    /// Creates a child signal: cancelling this signal cancels the child, so
    /// one owner can fan out one signal per operation while all of them
    /// terminate together.
    #[must_use]
    pub fn child(&self) -> Self {
        Self {
            token: self.token.child_token(),
            commit_gate: self.commit_gate.clone(),
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
