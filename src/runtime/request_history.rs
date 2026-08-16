//! In-memory ownership of settled provider-neutral request facts.
//!
//! `MessageLedger` owns canonical conversational facts, and
//! `ConversationState` owns historical Surface revisions. This type owns only
//! the frozen non-history [`RequestSnapshot`] values that were admitted by
//! the Agent Loop. It is intentionally an append-only runtime coordination
//! seam, not a second transcript or a persistence framework.
//!
//! The type is runtime-owned semantic state (Issue #61): the conversation
//! runtime coordinator appends settled attempt snapshots under its own
//! lock and serves read-only clones. It is **not** Runtime Client
//! projection state and never lives in the projection read model; the
//! Runtime Client adapter reads and reconstructs through the runtime.

use crate::conversation::ConversationState;
use crate::model::{ModelRequest, RequestIdentity, RequestReconstructionError, RequestSnapshot};

/// The in-memory historical request-fact owner of one conversation runtime.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RequestHistory {
    snapshots: Vec<RequestSnapshot>,
}

/// A request-history lookup or append failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequestHistoryError {
    /// No retained snapshot has this request identity.
    NotFound(RequestIdentity),
    /// The Agent Loop attempted to append the same actual request identity
    /// more than once. This is a coordination defect, not content-based
    /// deduplication.
    DuplicateIdentity(RequestIdentity),
    /// Historical Surface ownership is temporarily unavailable while the
    /// runtime has moved the single `ConversationState` into a running
    /// attempt.
    ConversationUnavailable,
    /// The retained snapshot could not hydrate its referenced Surface
    /// revision.
    Reconstruction(RequestReconstructionError),
}

impl core::fmt::Display for RequestHistoryError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::NotFound(identity) => write!(f, "request snapshot not found: {identity:?}"),
            Self::DuplicateIdentity(identity) => {
                write!(
                    f,
                    "request snapshot identity was appended twice: {identity:?}"
                )
            }
            Self::ConversationUnavailable => {
                f.write_str("historical ConversationState is owned by a running attempt")
            }
            Self::Reconstruction(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for RequestHistoryError {}

impl From<RequestReconstructionError> for RequestHistoryError {
    fn from(error: RequestReconstructionError) -> Self {
        Self::Reconstruction(error)
    }
}

impl RequestHistory {
    /// The retained actual provider-request snapshots in admission order.
    #[must_use]
    pub fn snapshots(&self) -> &[RequestSnapshot] {
        &self.snapshots
    }

    /// Looks up one immutable request snapshot by its actual request identity.
    #[must_use]
    pub fn get(&self, identity: &RequestIdentity) -> Option<&RequestSnapshot> {
        self.snapshots
            .iter()
            .find(|snapshot| snapshot.identity == *identity)
    }

    /// Appends actual request snapshots exactly once, preserving provider
    /// request order. The whole batch is checked before mutation.
    pub(crate) fn append(
        &mut self,
        snapshots: impl IntoIterator<Item = RequestSnapshot>,
    ) -> Result<(), RequestHistoryError> {
        let incoming = snapshots.into_iter().collect::<Vec<_>>();
        for (index, snapshot) in incoming.iter().enumerate() {
            if self.get(&snapshot.identity).is_some()
                || incoming[..index]
                    .iter()
                    .any(|previous| previous.identity == snapshot.identity)
            {
                return Err(RequestHistoryError::DuplicateIdentity(
                    snapshot.identity.clone(),
                ));
            }
        }
        self.snapshots.extend(incoming);
        Ok(())
    }

    /// Reconstructs the exact provider-neutral request from one retained
    /// snapshot and the authoritative historical `ConversationState`.
    ///
    /// # Errors
    ///
    /// Returns [`RequestHistoryError::NotFound`] for an unknown request
    /// identity or [`RequestHistoryError::Reconstruction`] when its Surface
    /// revision cannot be hydrated.
    pub fn reconstruct(
        &self,
        identity: &RequestIdentity,
        conversation: &ConversationState,
    ) -> Result<ModelRequest, RequestHistoryError> {
        self.get(identity)
            .ok_or_else(|| RequestHistoryError::NotFound(identity.clone()))?
            .reconstruct(conversation)
            .map_err(RequestHistoryError::from)
    }
}
