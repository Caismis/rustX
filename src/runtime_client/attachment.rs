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
    /// Read the first bounded page for one exact Job identity.
    /// # Errors
    /// Closed attachments, unavailable jobs, and expired native cuts are explicit.
    pub fn job_status(
        &self,
        id: &crate::runtime::identity::ToolExecutionId,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        self.access(false)?
            .domain_page(&format!("jobs.{id}"), 0, 32)
    }
    /// Read the finite Job listing through the native owner.
    /// # Errors
    /// A closed attachment is explicit.
    pub fn job_list(&self) -> Result<RuntimeClientResult, RuntimeClientError> {
        self.access(false)?.job_list()
    }
    native_control!(agent_status, false, id: &crate::runtime::identity::AgentId);
    native_control!(agent_list, false);
    /// Read durable child meters without blocking an async dispatch worker.
    /// # Errors
    /// Closed attachments and unavailable durable evidence are explicit.
    pub async fn agent_statistics(
        &self,
        id: &crate::runtime::identity::AgentId,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        self.access(false)?.agent_statistics(id).await
    }
    /// Read a child's complete native turn directory in bounded pages.
    /// # Errors
    /// Rejects closed attachments, unknown children and invalid page bounds.
    pub fn agent_turns(
        &self,
        id: &crate::runtime::identity::AgentId,
        offset: Option<usize>,
        limit: usize,
    ) -> Result<crate::runtime_client::turn::ObservedTurnPage, RuntimeClientError> {
        self.access(false)?.agent_turns(id, offset, limit)
    }
    /// Locate one child Turn or page around its captured native cut.
    /// # Errors
    /// Rejects foreign identities, invalid cuts and unavailable history.
    pub fn agent_transcript_window(
        &self,
        id: &crate::runtime::identity::AgentId,
        at: &crate::durable::reading::ConversationWindowAt,
        limit: usize,
    ) -> Result<super::snapshot::ConversationWindow, RuntimeClientError> {
        self.access(false)?.agent_transcript_window(id, at, limit)
    }

    /// Wait for this exact finite Job's physical settlement.
    /// # Errors
    /// Closed attachments and unknown Jobs are rejected.
    pub async fn job_wait(
        &self,
        id: &crate::runtime::identity::ToolExecutionId,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        self.access(false)?.job_wait(id).await
    }
    /// Commit cancellation intent for this exact Job without awaiting physical settlement.
    /// # Errors
    /// Read-only attachments and unknown Jobs are rejected.
    pub fn job_cancel(
        &self,
        id: &crate::runtime::identity::ToolExecutionId,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        self.access(true)?.job_cancel_intent(id)
    }
    /// Read a coherent child-owned conversation cut, waiting after its cursor.
    /// # Errors
    /// Closed attachments, unknown Agents and unavailable live endpoints reject.
    pub async fn agent_observe(
        &self,
        id: &crate::runtime::identity::AgentId,
        after: Option<super::agent_observation::AgentObservationCursor>,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        self.access(false)?.agent_observe(id, after).await
    }
    /// Deliver to the active child or atomically admit its next activation.
    /// # Errors
    /// Read-only attachments, unknown Agents and stopping Agents are rejected.
    pub async fn agent_send_message(
        &self,
        id: &crate::runtime::identity::AgentId,
        message: String,
        attachments: Vec<crate::message::content::UploadedFileRef>,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        self.access(true)?
            .agent_send_message(id, message, attachments)
            .await
    }
    /// Capture and wait for the activation current at owner admission.
    /// # Errors
    /// Closed attachments and unknown Agents are rejected.
    pub async fn agent_wait(
        &self,
        id: &crate::runtime::identity::AgentId,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        self.access(false)?.agent_wait(id).await
    }
    /// Commit interruption intent for the exact reserved/current activation at owner admission.
    /// # Errors
    /// Read-only attachments, unknown Agents and unavailable native owners are rejected.
    pub fn agent_interrupt(
        &self,
        id: &crate::runtime::identity::AgentId,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        self.access(true)?.agent_interrupt_intent(id)
    }

    native_control!(model_get, false);
    native_control!(model_catalog, false);
    native_control!(capability, false);
    /// Replace a standalone conversation's in-memory model selection.
    /// Durable Sessions require their native Session owner instead.
    /// # Errors
    /// Closed/read-only attachments and Session-owned runtimes are rejected;
    /// invalid configurations leave the current selection unchanged.
    pub fn model_set(
        &self,
        config: crate::model::session::SessionModelConfig,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        self.access(true)?.model_set(config)
    }
    native_control!(goal_control, true, control: crate::goal::GoalControl);
    native_control!(trace_page, false, before: Option<super::trace::TraceCursor>, limit: usize, records: Vec<super::trace::TraceCursor>);
    native_control!(transcript_page, false, before: Option<super::snapshot::RuntimeClientTranscriptCursor>, limit: usize);

    /// Await the native maintenance operation.
    /// # Errors
    /// Closed attachment or rejected/failed compaction.
    pub async fn compact_context(
        &self,
        request_id: Option<crate::runtime::identity::ManualCompactionRequestId>,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        self.access(true)?.compact_context(request_id).await
    }

    /// Capture native operation authority at admission. Only a server-owned
    /// operation lease may retain this handle; it is not an external attachment.
    pub(crate) fn operation_authority(&self) -> Result<Arc<ClientInner>, RuntimeClientError> {
        self.access(true)
    }
    pub(crate) fn read_authority(&self) -> Result<Arc<ClientInner>, RuntimeClientError> {
        self.access(false)
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

    /// Read one bounded checkpoint from the native cut owner.
    /// # Errors
    /// Closed attachments, unavailable sources, and expired cuts are explicit.
    pub fn checkpoint(
        &self,
    ) -> Result<super::checkpoint::RuntimeClientCheckpoint, RuntimeClientError> {
        self.access(false)?.checkpoint()
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
        if self.read_only && (request.is_mutating() || !request.is_inspection_read()) {
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
            RuntimeClientRequest::Goal {
                control: crate::goal::GoalControl::Show,
                ..
            } => inner.domain_page("goal", 0, 32),
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
            RuntimeClientRequest::CheckpointGet { .. } => {
                inner
                    .checkpoint()
                    .map(|checkpoint| RuntimeClientResult::Checkpoint {
                        checkpoint: Box::new(checkpoint),
                    })
            }
            RuntimeClientRequest::TracePageGet {
                before,
                limit,
                records,
                ..
            } => inner.trace_page(before, limit, records),
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
            RuntimeClientRequest::ContentRead {
                content_ref,
                offset,
                max_bytes,
                ..
            } => inner
                .content_read(&content_ref, offset, max_bytes)
                .map(|chunk| RuntimeClientResult::ContentChunk { chunk }),
            RuntimeClientRequest::CheckpointPageGet {
                cut,
                domain,
                offset,
                limit,
                ..
            } => inner
                .checkpoint_page(&cut, &domain, offset, limit)
                .map(|page| RuntimeClientResult::CheckpointPage { page }),
            RuntimeClientRequest::CheckpointValueRead {
                cut,
                domain,
                item_offset,
                path,
                offset,
                max_bytes,
                ..
            } => inner
                .checkpoint_value_read(&cut, &domain, item_offset, &path, offset, max_bytes)
                .map(|chunk| RuntimeClientResult::CheckpointValueChunk { chunk }),
            RuntimeClientRequest::InteractionPageGet {
                interaction,
                path,
                offset,
                limit,
                ..
            } => inner.interaction_fields(&interaction, &path, offset, limit),
            RuntimeClientRequest::InteractionValueRead {
                interaction,
                path,
                offset,
                max_bytes,
                ..
            } => inner.interaction_value_read(&interaction, &path, offset, max_bytes),
            RuntimeClientRequest::CapabilityGet { .. } => inner.domain_page("capabilities", 0, 32),
            RuntimeClientRequest::ModelCatalogGet { .. } => {
                inner.domain_page("model_catalog.models", 0, 32)
            }
            RuntimeClientRequest::ModelGet { .. } => inner.domain_page("model", 0, 32),
            RuntimeClientRequest::ModelSet { config, .. } => inner.model_set(*config),
            RuntimeClientRequest::JobStatus { job_id, .. } => inner.job_status_page(&job_id),
            RuntimeClientRequest::JobList { .. } => inner.domain_page("jobs", 0, 32),
            RuntimeClientRequest::AgentStatus { agent_id, .. } => {
                inner.domain_page(&format!("agents.{agent_id}"), 0, 32)
            }
            RuntimeClientRequest::AgentList { .. } => inner.domain_page("agents", 0, 32),
            RuntimeClientRequest::ObserveEvents { .. }
            | RuntimeClientRequest::AgentObserve { .. }
            | RuntimeClientRequest::AgentStatistics { .. }
            | RuntimeClientRequest::AgentTrace { .. }
            | RuntimeClientRequest::TraceLocateTool { .. }
            | RuntimeClientRequest::AgentTraceLocateTool { .. }
            | RuntimeClientRequest::AgentTraceDetail { .. } => {
                unreachable!("Agent statistics are read asynchronously")
            }
            RuntimeClientRequest::AgentTranscript {
                agent_id,
                at,
                limit,
                ..
            } => inner
                .agent_transcript_window(&agent_id, &at, limit)
                .map(|window| RuntimeClientResult::TranscriptWindow {
                    window: Box::new(window),
                }),
            RuntimeClientRequest::AgentTurns {
                agent_id,
                offset,
                limit,
                ..
            } => inner
                .agent_turns(&agent_id, offset, limit)
                .map(|page| RuntimeClientResult::ConversationTurns { page }),
            RuntimeClientRequest::JobWait { .. }
            | RuntimeClientRequest::JobCancel { .. }
            | RuntimeClientRequest::AgentSendMessage { .. }
            | RuntimeClientRequest::AgentWait { .. }
            | RuntimeClientRequest::AgentInterrupt { .. } => {
                unreachable!("Agent and Job settlement controls are awaited")
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
        if self.read_only && (request.is_mutating() || !request.is_inspection_read()) {
            return Self::error_response(
                id,
                RuntimeClientError::InvalidState {
                    message: "conversation inspection is read-only".to_owned(),
                },
            );
        }

        let domain_result = match &request {
            RuntimeClientRequest::TraceLocateTool { locator, .. } => {
                Some(inner.trace_locate_tool(locator.clone()).await)
            }
            RuntimeClientRequest::InteractionPageGet {
                interaction,
                path,
                offset,
                limit,
                ..
            } => Some(
                inner
                    .interaction_fields_async(interaction, path, *offset, *limit)
                    .await,
            ),
            RuntimeClientRequest::InteractionValueRead {
                interaction,
                path,
                offset,
                max_bytes,
                ..
            } => Some(
                inner
                    .interaction_value_read_async(interaction, path, (*offset, *max_bytes))
                    .await,
            ),
            RuntimeClientRequest::ObserveEvents { after_cursor, .. } => Some(
                inner
                    .observe_events(&self.attachment_id, *after_cursor)
                    .await,
            ),
            RuntimeClientRequest::AgentObserve {
                agent_id, after, ..
            } => Some(inner.agent_observe(agent_id, after.clone()).await),
            RuntimeClientRequest::AgentStatistics { agent_id, .. } => {
                Some(inner.agent_statistics(agent_id).await)
            }
            RuntimeClientRequest::AgentTrace {
                agent_id,
                before,
                limit,
                records,
                ..
            } => Some(
                inner
                    .agent_trace(agent_id, before.clone(), *limit, records.clone())
                    .await,
            ),
            RuntimeClientRequest::AgentTraceDetail {
                agent_id,
                record_id,
                ..
            } => Some(inner.agent_trace_detail(agent_id, record_id.clone()).await),
            RuntimeClientRequest::AgentTraceLocateTool {
                agent_id, locator, ..
            } => Some(
                inner
                    .agent_trace_locate_tool(agent_id, locator.clone())
                    .await,
            ),
            RuntimeClientRequest::JobWait { job_id, .. } => Some(inner.job_wait(job_id).await),
            RuntimeClientRequest::JobCancel { job_id, .. } => Some(inner.job_cancel_intent(job_id)),
            RuntimeClientRequest::AgentSendMessage {
                agent_id,
                message,
                attachments,
                ..
            } => Some(
                inner
                    .agent_send_message(agent_id, message.clone(), attachments.clone())
                    .await,
            ),
            RuntimeClientRequest::AgentWait { agent_id, .. } => {
                Some(inner.agent_wait(agent_id).await)
            }
            RuntimeClientRequest::AgentInterrupt { agent_id, .. } => {
                Some(inner.agent_interrupt_intent(agent_id))
            }
            _ => None,
        };
        if let Some(result) = domain_result {
            return match result {
                Ok(result) => RuntimeClientResponse {
                    id,
                    result: Some(result),
                    error: None,
                },
                Err(error) => Self::error_response(id, error),
            };
        }
        if !matches!(request, RuntimeClientRequest::Shutdown { .. }) {
            if matches!(request, RuntimeClientRequest::CompactContext { .. }) {
                let result = inner.compact_context(None).await;
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
    /// asked for the repair. The host registration is authoritative: its
    /// replacement precedes publication of the attachment's local delivery
    /// handle, and consumers may wake inside that publication interval.
    ///
    /// # Panics
    ///
    /// Panics only if the native host state lock is poisoned.
    #[must_use]
    pub fn superseded(&self, observed: &EventSubscription) -> bool {
        self.inner
            .upgrade()
            .is_some_and(|inner| inner.subscription_superseded(&self.attachment_id, observed))
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
