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
//! history, capability state) untouched. The attachment observes and
//! controls the conversation runtime through the host's projection/control
//! adapter; it owns no semantic runtime state itself.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use super::host::{ClientInner, EventDelivery, EventSubscription};
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
    /// The shared Runtime Client host state.
    inner: Arc<ClientInner>,
    /// Whether this handle already detached explicitly.
    detached: AtomicBool,
    /// The active event subscription (created by the subscribe path),
    /// polled by `next_event` / `try_next_event`.
    subscription: Mutex<Option<EventSubscription>>,
}

impl RuntimeAttachment {
    /// Creates the attachment handle over the shared host state.
    pub(crate) fn new(attachment_id: AttachmentId, inner: Arc<ClientInner>) -> Self {
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
        if request.requires_async() {
            return Self::error_response(
                id,
                RuntimeClientError::InvalidRequest {
                    message: "this control request must be awaited through handle_request_async"
                        .to_owned(),
                },
            );
        }
        let result = match request {
            RuntimeClientRequest::Initialize { .. } => Err(RuntimeClientError::InvalidRequest {
                message: "the attachment is already initialized".to_owned(),
            }),
            RuntimeClientRequest::SubmitInbound { content, .. } => {
                self.inner.submit_inbound(content)
            }
            RuntimeClientRequest::CancelCurrentAttempt { .. } => {
                self.inner.cancel_current_attempt()
            }
            RuntimeClientRequest::CompactContext { .. } => {
                unreachable!("manual compaction is handled asynchronously")
            }
            RuntimeClientRequest::InteractionRespond {
                interaction_id,
                response,
                ..
            } => self.inner.respond_interaction(&interaction_id, response),
            RuntimeClientRequest::SnapshotGet { .. } => self
                .inner
                .snapshot()
                .map(|(snapshot, cursor)| RuntimeClientResult::Snapshot { snapshot, cursor }),
            RuntimeClientRequest::SubscribeEvents { after_cursor, .. } => {
                match self
                    .inner
                    .subscribe_events(&self.attachment_id, after_cursor)
                {
                    Ok((subscription, result)) => {
                        self.store_subscription(subscription);
                        Ok(result)
                    }
                    Err(error) => Err(error),
                }
            }
            RuntimeClientRequest::CapabilityGet { .. } => self.inner.capability(),
            RuntimeClientRequest::ModelCatalogGet { .. } => self.inner.model_catalog(),
            RuntimeClientRequest::ModelGet { .. } => self.inner.model_get(),
            RuntimeClientRequest::ModelSet { config, .. } => self.inner.model_set(*config),
            RuntimeClientRequest::ApprovalModeSet { mode, .. } => {
                self.inner.approval_mode_set(mode)
            }
            RuntimeClientRequest::SessionList { .. }
            | RuntimeClientRequest::SessionGet { .. }
            | RuntimeClientRequest::SessionTreeGet { .. }
            | RuntimeClientRequest::SessionName { .. }
            | RuntimeClientRequest::SessionNew { .. }
            | RuntimeClientRequest::SessionSelect { .. }
            | RuntimeClientRequest::SessionClone { .. }
            | RuntimeClientRequest::SessionFork { .. }
            | RuntimeClientRequest::SessionTreeBranch { .. } => {
                unreachable!("native Session requests are handled asynchronously")
            }
            RuntimeClientRequest::BackgroundStatus { execution_id, .. } => {
                self.inner.background_status(&execution_id)
            }
            RuntimeClientRequest::BackgroundCancel { execution_id, .. } => {
                self.inner.background_cancel(&execution_id)
            }
            RuntimeClientRequest::SubagentStatus { subagent_id, .. } => {
                self.inner.subagent_status(&subagent_id)
            }
            RuntimeClientRequest::SubagentCancel { subagent_id, .. } => {
                self.inner.subagent_cancel(&subagent_id)
            }
            RuntimeClientRequest::Detach { .. } => {
                self.detach();
                Ok(RuntimeClientResult::Detached)
            }
            RuntimeClientRequest::Shutdown { .. } => unreachable!("shutdown handled above"),
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

    /// Handles a request whose semantic operation may await runtime-owned
    /// settlement. In particular, a successful shutdown response means the
    /// conversation runtime is already quiescent.
    pub async fn handle_request_async(
        &self,
        request: RuntimeClientRequest,
    ) -> RuntimeClientResponse {
        let id = request.id();
        if self.detached.load(Ordering::SeqCst) {
            return Self::error_response(id, RuntimeClientError::NotAttached);
        }
        if !matches!(request, RuntimeClientRequest::Shutdown { .. }) {
            if matches!(request, RuntimeClientRequest::CompactContext { .. }) {
                let result = self.inner.compact_context().await;
                return match result {
                    Ok(result) => RuntimeClientResponse {
                        id,
                        result: Some(result),
                        error: None,
                    },
                    Err(error) => Self::error_response(id, error),
                };
            }
            if let Some(session_request) = request.session_request() {
                let result = self.inner.session_request(session_request).await;
                return match result {
                    Ok(result) => RuntimeClientResponse {
                        id,
                        result: Some(result),
                        error: None,
                    },
                    Err(error) => Self::error_response(id, error),
                };
            }
            return self.handle_request(request);
        }
        let result = self.inner.shutdown().await;
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
        match self
            .inner
            .subscribe_events(&self.attachment_id, after_cursor)
        {
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
        self.inner.detach(&self.attachment_id);
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
            self.inner.detach(&self.attachment_id);
        }
    }
}
