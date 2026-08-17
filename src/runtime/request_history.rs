//! On-demand durable access to historical Request Snapshots.
//!
//! This type is a read handle, not a `Vec<RequestSnapshot>` authority. The
//! immutable snapshot and the historical Surface/Ledger it references live
//! in the conversation store; callers receive a bounded read result only
//! when they ask for it.

use std::sync::Arc;

use crate::durable::{ConversationStore, ConversationStoreError};
use crate::model::{ModelRequest, RequestIdentity, RequestSnapshot};

/// A durable request-history read handle for one conversation.
#[derive(Clone)]
pub struct RequestHistory {
    store: Arc<dyn ConversationStore>,
}

impl std::fmt::Debug for RequestHistory {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("RequestHistory(durable-read-handle)")
    }
}

impl PartialEq for RequestHistory {
    fn eq(&self, other: &Self) -> bool {
        self.snapshots() == other.snapshots()
    }
}

impl Eq for RequestHistory {}

/// A request-history lookup or durable reconstruction failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequestHistoryError {
    /// No retained snapshot has this request identity.
    NotFound(RequestIdentity),
    /// The retained snapshot or its referenced Surface/Ledger facts failed
    /// to reconstruct.
    Reconstruction(String),
}

impl std::fmt::Display for RequestHistoryError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound(identity) => {
                write!(formatter, "request snapshot not found: {identity:?}")
            }
            Self::Reconstruction(error) => formatter.write_str(error),
        }
    }
}

impl std::error::Error for RequestHistoryError {}

impl RequestHistory {
    /// Creates a read handle over the conversation store.
    #[must_use]
    pub fn new(store: Arc<dyn ConversationStore>) -> Self {
        Self { store }
    }

    /// Loads retained snapshots on demand. The returned vector is a bounded
    /// read result and is not kept by the runtime.
    ///
    /// # Panics
    ///
    /// Panics if the durable store rejects the read. Runtime production paths
    /// use `reconstruct` for fallible historical reads; this convenience is
    /// intended for projection/debug views and tests.
    #[must_use]
    pub fn snapshots(&self) -> Vec<RequestSnapshot> {
        self.store
            .list_request_snapshots()
            .expect("durable request snapshot read failed")
    }

    /// Looks up one immutable snapshot on demand.
    ///
    /// # Errors
    ///
    /// Returns [`RequestHistoryError::Reconstruction`] when the durable
    /// authority is corrupt or unavailable. Durable read failures are never
    /// converted into a misleading `None`.
    pub fn get(
        &self,
        identity: &RequestIdentity,
    ) -> Result<Option<RequestSnapshot>, RequestHistoryError> {
        self.get_durable(identity)
    }

    fn get_durable(
        &self,
        identity: &RequestIdentity,
    ) -> Result<Option<RequestSnapshot>, RequestHistoryError> {
        match self.store.load_request_snapshot(&identity.request_id()) {
            Ok(snapshot) if snapshot.identity == *identity => Ok(Some(snapshot)),
            Ok(_) => Err(RequestHistoryError::Reconstruction(
                "durable Request Snapshot identity disagrees with its lookup key".to_owned(),
            )),
            Err(ConversationStoreError::RequestNotFound(_)) => Ok(None),
            Err(error) => Err(RequestHistoryError::Reconstruction(error.to_string())),
        }
    }

    /// Reconstructs the exact provider-neutral request from durable facts.
    ///
    /// # Errors
    ///
    /// Returns [`RequestHistoryError::NotFound`] when the identity has no
    /// retained snapshot, or [`RequestHistoryError::Reconstruction`] when the
    /// immutable Surface/Ledger references cannot be read.
    pub fn reconstruct(
        &self,
        identity: &RequestIdentity,
    ) -> Result<ModelRequest, RequestHistoryError> {
        let snapshot = self
            .get_durable(identity)?
            .ok_or_else(|| RequestHistoryError::NotFound(identity.clone()))?;
        self.store
            .reconstruct_model_request(&snapshot.request_id)
            .map_err(|error| RequestHistoryError::Reconstruction(error.to_string()))
    }
}
