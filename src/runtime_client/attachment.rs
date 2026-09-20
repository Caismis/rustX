//! The Runtime Client attachment: one active client session of the protocol.
//!
//! An attachment is the semantic session of one client. A host admits one
//! control-capable attachment and any number of explicitly read-only
//! observation attachments:
//!
//! - the first attachment succeeds;
//! - a second simultaneous control attachment fails deterministically
//!   (`attachment_in_use`) and never evicts the first;
//! - read-only observation attachments do not compete with the control
//!   attachment or with one another;
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
use std::sync::{Arc, Mutex, Weak};

use super::host::{ClientInner, EventDelivery, EventSubscription};
use super::types::{
    AttachmentId, RuntimeClientError, RuntimeClientRequest, RuntimeClientResponse,
    RuntimeClientResult,
};

/// Native attachment admission facts, without version negotiation or RPC envelopes.
pub struct AttachedSnapshot {
    pub attachment: RuntimeAttachment,
    pub snapshot: super::snapshot::RuntimeClientSnapshot,
    pub cursor: super::types::RuntimeClientCursor,
}

macro_rules! native_control {
    ($name:ident, $write:expr $(, $argument:ident : $ty:ty)*) => {
        /// Invoke the existing native control owner through this attachment.
        /// # Errors
        /// Closed attachments and native validation failures are explicit.
        pub fn $name(&self, $($argument: $ty),*) -> Result<RuntimeClientResult, RuntimeClientError> {
            self.access($write)?.$name($($argument),*)
        }
    };
}

/// One admitted Runtime Client attachment.
///
/// The handle is RAII: dropping it releases the attachment (the same
/// semantics as an explicit detach, and never anything more). The handle
/// is intended for use by one task at a time.
pub struct RuntimeAttachment {
    /// The attachment identity.
    attachment_id: AttachmentId,
    /// Residency owns the host. An external handle cannot extend its lifetime.
    inner: Weak<ClientInner>,
    /// Whether this attachment can only observe the projection.
    read_only: bool,
    /// Whether this handle already detached explicitly.
    detached: AtomicBool,
    /// The active event subscription (created by the subscribe path),
    /// polled by `next_event` / `try_next_event`.
    subscription: Mutex<Option<EventSubscription>>,
}

impl RuntimeAttachment {
    native_control!(model_get, false);
    native_control!(model_catalog, false);
    native_control!(capability, false);
    native_control!(model_set, true, config: crate::model::session::SessionModelConfig);
    native_control!(goal_control, true, control: crate::goal::GoalControl);
    native_control!(trace_page, false, before: Option<super::trace::TraceCursor>, limit: usize);
    native_control!(transcript_page, false, before: Option<super::snapshot::RuntimeClientTranscriptCursor>, limit: usize);
    native_control!(background_status, false, id: &crate::runtime::identity::ToolExecutionId);
    native_control!(background_cancel, true, id: &crate::runtime::identity::ToolExecutionId);
    native_control!(subagent_transcript_page, false, id: &crate::runtime::identity::SubagentId, before: Option<super::snapshot::RuntimeClientTranscriptCursor>, limit: usize);
    native_control!(subagent_status, false, id: &crate::runtime::identity::SubagentId);
    native_control!(subagent_cancel, true, id: &crate::runtime::identity::SubagentId);

    /// Await the native maintenance operation.
    /// # Errors
    /// Closed attachment or rejected/failed compaction.
    pub async fn compact_context(&self) -> Result<RuntimeClientResult, RuntimeClientError> {
        self.access(true)?.compact_context().await
    }

    /// Dispose only the retained resource identified by the native subagent owner.
    /// # Errors
    /// Closed attachment or native disposal refusal/failure.
    pub async fn subagent_workspace_dispose(
        &self,
        id: &crate::runtime::identity::SubagentId,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        self.access(true)?.subagent_workspace_dispose(id).await
    }
    /// Capture native operation authority at admission. Only a server-owned
    /// operation lease may retain this handle; it is not an external attachment.
    pub(crate) fn operation_authority(&self) -> Result<Arc<ClientInner>, RuntimeClientError> {
        self.access(true)
    }

    fn access(&self, write: bool) -> Result<Arc<ClientInner>, RuntimeClientError> {
        if self.detached.load(Ordering::SeqCst) {
            return Err(RuntimeClientError::NotAttached);
        }
        if write && self.read_only {
            return Err(RuntimeClientError::InvalidState {
                message: "inspection attachment is read-only".into(),
            });
        }
        self.inner.upgrade().ok_or(RuntimeClientError::NotAttached)
    }

    /// Submit through native admission; no request/correlation envelope is involved.
    /// # Errors
    /// Closed attachments and native admission failures are explicit.
    pub fn submit_inbound(
        &self,
        content: Vec<crate::message::types::UserContentBlock>,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        self.access(true)?.submit_inbound(content)
    }

    /// Request cancellation through the native attempt owner.
    /// # Errors
    /// Closed attachments or no cancellable attempt.
    pub fn cancel_current_attempt(&self) -> Result<RuntimeClientResult, RuntimeClientError> {
        self.access(true)?.cancel_current_attempt()
    }

    /// Read the linearized native projection.
    /// # Errors
    /// Closed attachments or projection failures.
    pub fn snapshot(&self) -> Result<RuntimeClientResult, RuntimeClientError> {
        self.access(false)?
            .snapshot()
            .map(|(snapshot, cursor)| RuntimeClientResult::Snapshot { snapshot, cursor })
    }

    /// Answer through the runtime-owned interaction coordinator.
    /// # Errors
    /// Closed attachments, stale interaction or invalid response.
    pub async fn respond_interaction(
        &self,
        interaction: &crate::runtime::interaction::InteractionRef,
        response: crate::runtime::interaction::InteractionResponse,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        self.access(true)?
            .respond_interaction(interaction, response)
            .await
    }

    /// Cancel exactly one pending interaction at its authoritative coordinator.
    /// # Errors
    /// Closed attachments and stale interactions fail explicitly.
    pub async fn cancel_interaction(
        &self,
        interaction: &crate::runtime::interaction::InteractionRef,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        self.access(true)?.cancel_interaction(interaction).await
    }
    /// Creates the attachment handle over the shared host state.
    pub(crate) fn new(
        attachment_id: AttachmentId,
        inner: &Arc<ClientInner>,
        read_only: bool,
    ) -> Self {
        Self {
            attachment_id,
            inner: Arc::downgrade(inner),
            read_only,
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
    #[allow(clippy::too_many_lines)] // request dispatch remains one semantic boundary
    pub fn handle_request(&self, request: RuntimeClientRequest) -> RuntimeClientResponse {
        let id = request.id();
        let Some(inner) = self.inner.upgrade() else {
            return Self::error_response(id, RuntimeClientError::NotAttached);
        };
        if self.detached.load(Ordering::SeqCst) {
            return Self::error_response(id, RuntimeClientError::NotAttached);
        }
        if self.read_only && request.is_mutating() {
            return Self::error_response(
                id,
                RuntimeClientError::InvalidState {
                    message: "conversation inspection is read-only".to_owned(),
                },
            );
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
            RuntimeClientRequest::Goal { control, .. } => inner.goal_control(control),
            RuntimeClientRequest::Initialize { .. } => Err(RuntimeClientError::InvalidRequest {
                message: "the attachment is already initialized".to_owned(),
            }),
            RuntimeClientRequest::SubmitInbound { content, .. } => inner.submit_inbound(content),
            RuntimeClientRequest::CancelCurrentAttempt { .. } => inner.cancel_current_attempt(),
            RuntimeClientRequest::CompactContext { .. } => {
                unreachable!("manual compaction is handled asynchronously")
            }
            RuntimeClientRequest::InteractionRespond { .. } => {
                unreachable!("interaction responses are handled asynchronously")
            }
            RuntimeClientRequest::SnapshotGet { .. } => inner
                .snapshot()
                .map(|(snapshot, cursor)| RuntimeClientResult::Snapshot { snapshot, cursor }),
            RuntimeClientRequest::TranscriptPageGet {
                before_cursor,
                limit,
                ..
            } => inner.transcript_page(before_cursor, limit),
            RuntimeClientRequest::SubscribeEvents { after_cursor, .. } => {
                match inner.subscribe_events(&self.attachment_id, after_cursor) {
                    Ok((subscription, result)) => {
                        self.store_subscription(subscription);
                        Ok(result)
                    }
                    Err(error) => Err(error),
                }
            }
            RuntimeClientRequest::CapabilityGet { .. } => inner.capability(),
            RuntimeClientRequest::ModelCatalogGet { .. } => inner.model_catalog(),
            RuntimeClientRequest::ModelGet { .. } => inner.model_get(),
            RuntimeClientRequest::ModelSet { config, .. } => inner.model_set(*config),
            RuntimeClientRequest::SessionDeletePreview { .. }
            | RuntimeClientRequest::SessionList { .. }
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
                inner.background_status(&execution_id)
            }
            RuntimeClientRequest::BackgroundCancel { execution_id, .. } => {
                inner.background_cancel(&execution_id)
            }
            RuntimeClientRequest::SubagentStatus { subagent_id, .. } => {
                inner.subagent_status(&subagent_id)
            }
            RuntimeClientRequest::SubagentCancel { subagent_id, .. } => {
                inner.subagent_cancel(&subagent_id)
            }
            RuntimeClientRequest::SubagentWorkspaceDispose { .. } => {
                unreachable!("retained workspace disposal is handled asynchronously")
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
    #[allow(clippy::too_many_lines)] // Exhaustive typed protocol dispatch.
    pub async fn handle_request_async(
        &self,
        request: RuntimeClientRequest,
    ) -> RuntimeClientResponse {
        let id = request.id();
        let Some(inner) = self.inner.upgrade() else {
            return Self::error_response(id, RuntimeClientError::NotAttached);
        };
        if self.detached.load(Ordering::SeqCst) {
            return Self::error_response(id, RuntimeClientError::NotAttached);
        }
        if self.read_only && request.is_mutating() {
            return Self::error_response(
                id,
                RuntimeClientError::InvalidState {
                    message: "conversation inspection is read-only".to_owned(),
                },
            );
        }

        if !matches!(request, RuntimeClientRequest::Shutdown { .. }) {
            if matches!(request, RuntimeClientRequest::CompactContext { .. }) {
                let result = inner.compact_context().await;
                return match result {
                    Ok(result) => RuntimeClientResponse {
                        id,
                        result: Some(result),
                        error: None,
                    },
                    Err(error) => Self::error_response(id, error),
                };
            }
            if let RuntimeClientRequest::InteractionRespond {
                interaction,
                response,
                ..
            } = &request
            {
                let result = inner
                    .respond_interaction(interaction, response.clone())
                    .await;
                return match result {
                    Ok(result) => RuntimeClientResponse {
                        id,
                        result: Some(result),
                        error: None,
                    },
                    Err(error) => Self::error_response(id, error),
                };
            }
            if let RuntimeClientRequest::SubagentWorkspaceDispose { subagent_id, .. } = &request {
                let result = inner.subagent_workspace_dispose(subagent_id).await;
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
                let result = inner.session_request(session_request).await;
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
        let result = inner.shutdown().await;
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
        let inner = self
            .inner
            .upgrade()
            .ok_or(RuntimeClientError::NotAttached)?;
        if self.detached.load(Ordering::SeqCst) {
            return Err(RuntimeClientError::NotAttached);
        }
        match inner.subscribe_events(&self.attachment_id, after_cursor) {
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

    /// Whether this exact registration has been replaced by a later one.
    ///
    /// Resync legitimately re-subscribes while a consumer is parked on the
    /// previous registration. Without this, that consumer's
    /// [`EventDelivery::Closed`] would be indistinguishable from the end of
    /// residency, and repairing a projection would retire the attachment that
    /// asked for the repair.
    ///
    /// # Panics
    ///
    /// Panics only if the attachment subscription lock is poisoned, which
    /// would mean a previous operation panicked while holding the lock.
    #[must_use]
    pub fn superseded(&self, observed: &EventSubscription) -> bool {
        self.subscription
            .lock()
            .expect("attachment subscription lock poisoned")
            .as_ref()
            .is_some_and(|current| !current.same_registration(observed))
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
        if let Some(inner) = self.inner.upgrade() {
            inner.detach(&self.attachment_id);
        }
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
    pub(crate) fn store_subscription(&self, subscription: EventSubscription) {
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
        if !self.detached.swap(true, Ordering::SeqCst)
            && let Some(inner) = self.inner.upgrade()
        {
            inner.detach(&self.attachment_id);
        }
    }
}
