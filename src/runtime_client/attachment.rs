//! The Runtime Client attachment: the at-most-one active client session of
//! Protocol v1.
//!
//! An attachment is the semantic session of one client. Protocol v1 admits
//! exactly one active attachment per runtime instance:
//!
//! - the first attachment succeeds;
//! - a second simultaneous attachment fails deterministically
//!   (`attachment_in_use`) and never evicts the first;
//! - explicit detach (or drop) releases attachment ownership;
//! - reconnecting always receives a new attachment identity;
//! - request ids are scoped to one attachment and never carry across;
//! - cursor/replay state belongs to the runtime observation stream, not
//!   to the attachment: a detached client resuming later subscribes after
//!   the cursor it last observed.
//!
//! Detaching an attachment is **never** cancellation: it changes only
//! attachment state and leaves every semantic runtime fact (the current
//! attempt, conversation-owned background work, mailbox contents, canonical
//! history, capability state) untouched.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use super::host::{EventDelivery, EventSubscription, HostInner};
use super::types::{
    AttachmentId, RuntimeClientError, RuntimeClientRequest, RuntimeClientResponse,
    RuntimeClientResult,
};

/// One admitted Runtime Client attachment.
///
/// The handle is RAII: dropping it releases the attachment (the same
/// semantics as an explicit detach, and never anything more). The handle
/// is intended for use by one task at a time.
pub struct RuntimeAttachment {
    /// The attachment identity.
    attachment_id: AttachmentId,
    /// The shared host state.
    inner: Arc<HostInner>,
    /// Whether this handle already detached explicitly.
    detached: AtomicBool,
    /// The active event subscription (created by the subscribe path),
    /// polled by `next_event` / `try_next_event`.
    subscription: Mutex<Option<EventSubscription>>,
}

impl RuntimeAttachment {
    /// Creates the attachment handle over the shared host state.
    pub(crate) fn new(attachment_id: AttachmentId, inner: Arc<HostInner>) -> Self {
        Self {
            attachment_id,
            inner,
            detached: AtomicBool::new(false),
            subscription: Mutex::new(None),
        }
    }

    /// The attachment identity.
    #[must_use]
    pub fn attachment_id(&self) -> &AttachmentId {
        &self.attachment_id
    }

    /// Handles one client-initiated request and returns its correlated
    /// response.
    ///
    /// The request id is echoed exactly, so responses correlate even under
    /// request pipelining. Notifications never fabricate request ids.
    ///
    /// The `initialize` method is the one semantic operation this handle
    /// cannot serve: admission happened when the attachment was created,
    /// and re-initializing is an `invalid_request`.
    pub fn handle_request(&self, request: RuntimeClientRequest) -> RuntimeClientResponse {
        let id = request.id();
        if self.detached.load(Ordering::SeqCst) {
            return Self::error_response(id, RuntimeClientError::NotAttached);
        }
        let host = super::host::RuntimeClientHost {
            inner: self.inner.clone(),
        };
        let result = match request {
            RuntimeClientRequest::Initialize { .. } => Err(RuntimeClientError::InvalidRequest {
                message: "the attachment is already initialized".to_owned(),
            }),
            RuntimeClientRequest::SubmitInbound { content, .. } => host.submit_inbound(content),
            RuntimeClientRequest::CancelCurrentAttempt { .. } => host.cancel_current_attempt(),
            RuntimeClientRequest::SnapshotGet { .. } => host
                .snapshot()
                .map(|(snapshot, cursor)| RuntimeClientResult::Snapshot { snapshot, cursor }),
            RuntimeClientRequest::SubscribeEvents { after_cursor, .. } => {
                match host.subscribe_events(&self.attachment_id, after_cursor) {
                    Ok((subscription, result)) => {
                        self.store_subscription(subscription);
                        Ok(result)
                    }
                    Err(error) => Err(error),
                }
            }
            RuntimeClientRequest::CapabilityGet { .. } => host.capability(),
            RuntimeClientRequest::ModelCatalogGet { .. } => host.model_catalog(),
            RuntimeClientRequest::ModelGet { .. } => host.model_get(),
            RuntimeClientRequest::ModelSet { config, .. } => host.model_set(*config),
            RuntimeClientRequest::BackgroundStatus { execution_id, .. } => {
                host.background_status(&execution_id)
            }
            RuntimeClientRequest::BackgroundCancel { execution_id, .. } => {
                host.background_cancel(&execution_id)
            }
            RuntimeClientRequest::Detach { .. } => {
                self.detach();
                Ok(RuntimeClientResult::Detached)
            }
            RuntimeClientRequest::Shutdown { .. } => Ok(host.shutdown()),
        };
        match result {
            Ok(result) => RuntimeClientResponse {
                id,
                result: Some(result),
                error: None,
            },
            Err(error) => Self::error_response(id, error),
        }
    }

    /// Subscribes to the observation stream after a serviceable cursor and
    /// returns the direct delivery handle.
    ///
    /// The subscription is also stored so `next_event` can poll it; a
    /// later re-subscription replaces it (the old channel closes).
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeClientError::NotAttached`] after detach and
    /// [`RuntimeClientError::ResyncRequired`] for an unserviceable cursor.
    ///
    /// # Panics
    ///
    /// Panics only if the attachment subscription lock is poisoned, which
    /// would mean a previous operation panicked while holding the lock.
    pub fn subscribe_events(
        &self,
        after_cursor: super::types::RuntimeClientCursor,
    ) -> Result<EventSubscription, RuntimeClientError> {
        if self.detached.load(Ordering::SeqCst) {
            return Err(RuntimeClientError::NotAttached);
        }
        let host = super::host::RuntimeClientHost {
            inner: self.inner.clone(),
        };
        match host.subscribe_events(&self.attachment_id, after_cursor) {
            Ok((subscription, _result)) => {
                self.store_subscription(subscription.clone());
                Ok(subscription)
            }
            Err(error) => Err(error),
        }
    }

    /// The delivery handle of the active subscription, when one exists.
    ///
    /// The handle shares the one registration; it exists so a transport can
    /// pump events without holding any attachment lock across an await.
    ///
    /// # Panics
    ///
    /// Panics only if the attachment subscription lock is poisoned, which
    /// would mean a previous operation panicked while holding the lock.
    #[must_use]
    pub fn subscription(&self) -> Option<EventSubscription> {
        self.subscription
            .lock()
            .expect("attachment subscription lock poisoned")
            .clone()
    }

    /// Waits for the next delivery of the active subscription.
    ///
    /// Returns [`EventDelivery::Closed`] when no subscription is active or
    /// the subscription was released (detach or re-subscription).
    ///
    /// # Panics
    ///
    /// Panics only if the attachment subscription lock is poisoned, which
    /// would mean a previous operation panicked while holding the lock.
    pub async fn next_event(&self) -> EventDelivery {
        let Some(subscription) = self.subscription() else {
            return EventDelivery::Closed;
        };
        subscription.next().await
    }

    /// Polls the active subscription without waiting.
    ///
    /// # Panics
    ///
    /// Panics only if the attachment subscription lock is poisoned, which
    /// would mean a previous operation panicked while holding the lock.
    #[must_use]
    pub fn try_next_event(&self) -> EventDelivery {
        match self.subscription() {
            Some(subscription) => subscription.try_next(),
            None => EventDelivery::Closed,
        }
    }

    /// Releases the attachment explicitly. Idempotent: a second detach
    /// (or a later drop) is a no-op. Detach is never cancellation.
    ///
    /// # Panics
    ///
    /// Panics only if the attachment subscription lock is poisoned, which
    /// would mean a previous operation panicked while holding the lock.
    pub fn detach(&self) {
        if self.detached.swap(true, Ordering::SeqCst) {
            return;
        }
        super::host::RuntimeClientHost {
            inner: self.inner.clone(),
        }
        .detach(&self.attachment_id);
        // Take the handle out under the attachment lock and drop it after
        // releasing that lock: dropping a subscription acquires the host
        // lock, and no path may hold the attachment lock across it.
        let previous = self
            .subscription
            .lock()
            .expect("attachment subscription lock poisoned")
            .take();
        drop(previous);
    }

    /// Stores the delivery handle of a fresh subscription, releasing any
    /// previous one outside the attachment lock (dropping a subscription
    /// acquires the host lock).
    fn store_subscription(&self, subscription: EventSubscription) {
        let previous = self
            .subscription
            .lock()
            .expect("attachment subscription lock poisoned")
            .replace(subscription);
        drop(previous);
    }

    /// Builds the correlated response of a failed request.
    fn error_response(
        id: super::types::RequestId,
        error: RuntimeClientError,
    ) -> RuntimeClientResponse {
        RuntimeClientResponse {
            id,
            result: None,
            error: Some(error),
        }
    }
}

impl Drop for RuntimeAttachment {
    fn drop(&mut self) {
        if !self.detached.swap(true, Ordering::SeqCst) {
            super::host::RuntimeClientHost {
                inner: self.inner.clone(),
            }
            .detach(&self.attachment_id);
        }
    }
}
