//! Attachment admission and the control surface forwarded to native owners.
//! Detach is never cancellation; read-only attachments never control.
#[cfg(test)]
use super::super::projection::views::background_view;
use super::{
    Arc, AttachmentId, AttachmentState, CancelAttemptError, ClientInner, InboundAdmissionError,
    InteractionRef, InteractionResponse, ManualCompactionError, ModelUpdateError,
    RUNTIME_CLIENT_PROTOCOL_VERSION, RoutedInteractionError, RuntimeClientError,
    RuntimeClientResult, SessionModelConfig, ToolExecutionId,
};

/// Shared parameter authority, checked before resolving child ownership or storage.
pub(super) fn agent_control_error(
    id: &crate::runtime::identity::AgentId,
    error: crate::runtime::subagent::AgentControlError,
) -> RuntimeClientError {
    match error {
        crate::runtime::subagent::AgentControlError::NotDelivered => {
            RuntimeClientError::AgentNotDelivered {
                agent_id: id.clone(),
            }
        }
        crate::runtime::subagent::AgentControlError::DeliveryUnknown => {
            RuntimeClientError::AgentDeliveryUnknown {
                agent_id: id.clone(),
            }
        }
        crate::runtime::subagent::AgentControlError::Stopping => {
            RuntimeClientError::AgentStopping {
                agent_id: id.clone(),
            }
        }
        crate::runtime::subagent::AgentControlError::Unknown(agent_id) => {
            RuntimeClientError::UnknownAgent { agent_id }
        }
        crate::runtime::subagent::AgentControlError::Settlement => {
            RuntimeClientError::AgentSettlement {
                agent_id: id.clone(),
            }
        }
        error => RuntimeClientError::InvalidState {
            message: error.to_string(),
        },
    }
}

impl ClientInner {
    pub(crate) fn workspace_changes(&self) -> Option<Arc<crate::workspace_changes::ChangesStore>> {
        self.runtime
            .as_ref()
            .map(|runtime| Arc::clone(&runtime.tool_runtime().workspace_changes))
    }
    /// Admits one attachment: the internal primitive behind the
    /// `initialize` protocol method.
    ///
    /// The Runtime Client host allows one control attachment. A second
    /// simultaneous control attach fails deterministically and never evicts
    /// the first; explicitly read-only inspection attachments use a separate
    /// subscriber set and may coexist with it. Initialization returns only
    /// the admitted identity and finite capabilities; a checkpoint is read
    /// separately against the current observation-source state.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeClientError::UnsupportedProtocolVersion`] for an
    /// unsupported version, [`RuntimeClientError::AttachmentInUse`] when the
    /// control attachment is active, and
    /// [`RuntimeClientError::ProjectionExhausted`] once the observation
    /// stream is over.
    pub(crate) fn attach(
        self: &Arc<Self>,
        protocol_version: u16,
    ) -> Result<
        (
            super::super::attachment::RuntimeAttachment,
            RuntimeClientResult,
        ),
        RuntimeClientError,
    > {
        self.attach_with_mode(protocol_version, false)
    }

    /// Admits one explicitly read-only attachment. Read-only attachments may
    /// coexist with the one control attachment and with one another; they
    /// consume only projection subscription state and never change provider
    /// admission or runtime lifecycle state.
    pub(crate) fn attach_read_only(
        self: &Arc<Self>,
        protocol_version: u16,
    ) -> Result<
        (
            super::super::attachment::RuntimeAttachment,
            RuntimeClientResult,
        ),
        RuntimeClientError,
    > {
        self.attach_with_mode(protocol_version, true)
    }

    pub(super) fn attach_with_mode(
        self: &Arc<Self>,
        protocol_version: u16,
        read_only_attachment: bool,
    ) -> Result<
        (
            super::super::attachment::RuntimeAttachment,
            RuntimeClientResult,
        ),
        RuntimeClientError,
    > {
        if protocol_version != RUNTIME_CLIENT_PROTOCOL_VERSION {
            return Err(RuntimeClientError::UnsupportedProtocolVersion {
                supported: RUNTIME_CLIENT_PROTOCOL_VERSION,
                requested: protocol_version,
            });
        }
        let attachment = self.admit_attachment_identity(read_only_attachment)?;
        let result = RuntimeClientResult::Initialized {
            attachment_id: attachment.attachment_id().clone(),
            conversation_id: self.conversation_id.clone(),
            agent_id: self.agent_id.clone(),
            activation_id: self
                .state
                .lock()
                .expect("native attachment scope")
                .projection
                .activation_id()
                .cloned(),
            protocol_version: RUNTIME_CLIENT_PROTOCOL_VERSION,
            read_only: self.read_only || read_only_attachment,
            capabilities: vec!["checkpoint".into(), "events".into(), "content_read".into()],
        };
        Ok((attachment, result))
    }

    /// Identity admission does no Store reads and materializes no display state.
    /// The Projection lock is the sole admission linearization point. Checkpoints
    /// and subscriptions are independent reads after this finite receipt.
    pub(crate) fn admit_attachment_identity(
        self: &Arc<Self>,
        read_only_attachment: bool,
    ) -> Result<super::super::attachment::RuntimeAttachment, RuntimeClientError> {
        self.ensure_worker();
        let read_only_attachment = self.read_only || read_only_attachment;
        let mut state = self.state.lock().expect("native projection lock");
        if !read_only_attachment && let Some(existing) = &state.control_attachment {
            return Err(RuntimeClientError::AttachmentInUse {
                existing_attachment_id: existing.attachment_id.clone(),
            });
        }
        let sequence = state
            .next_attachment_seq
            .checked_add(1)
            .ok_or(RuntimeClientError::ProjectionExhausted)?;
        let attachment_id = AttachmentId::new(format!("attachment-{sequence}"));
        let admission = AttachmentState {
            attachment_id: attachment_id.clone(),
            subscriber_id: None,
        };
        state.next_attachment_seq = sequence;
        if read_only_attachment {
            state
                .read_only_attachments
                .insert(attachment_id.clone(), admission);
        } else {
            state.control_attachment = Some(admission);
        }
        drop(state);
        Ok(super::super::attachment::RuntimeAttachment::new(
            attachment_id,
            self,
            read_only_attachment,
        ))
    }

    #[cfg(test)]
    pub(crate) fn inspection_subscriber_for_test(
        &self,
        attachment_id: &AttachmentId,
    ) -> Option<u64> {
        self.state
            .lock()
            .expect("native projection lock")
            .read_only_attachments
            .get(attachment_id)
            .and_then(|attachment| attachment.subscriber_id)
    }

    /// Releases one attachment. Idempotent: a second detach (or an
    /// attachment drop after an explicit detach) is a no-op. Detach is
    /// never cancellation and never shutdown.
    ///
    /// # Panics
    ///
    /// Panics only if the host lock is poisoned, which would mean a
    /// previous operation panicked while holding the lock.
    pub(crate) fn detach(&self, attachment_id: &AttachmentId) {
        let mut state = self.state.lock().expect("native projection lock");
        let control = state
            .control_attachment
            .as_ref()
            .is_some_and(|attachment| attachment.attachment_id == *attachment_id);
        let attachment = if control {
            state
                .control_attachment
                .take()
                .expect("the control attachment identity was just checked")
        } else {
            let Some(attachment) = state.read_only_attachments.remove(attachment_id) else {
                return;
            };
            attachment
        };
        if let Some(subscriber_id) = attachment.subscriber_id {
            state.projection.remove_subscriber(subscriber_id);
        }
    }

    /// Submits one inbound user message through the conversation runtime's
    /// single publish path.
    ///
    /// The runtime owns authoritative metadata: the message identity, the
    /// inbound sequence, the persisted timestamp, and the provenance are
    /// all runtime-assigned. Success means accepted/published, never
    /// assistant-finished: the runtime wake gate admits the next attempt
    /// when the runtime is idle, and while an attempt is running the
    /// message waits in the authoritative mailbox for the next safe-boundary
    /// drain. The host never admits an attempt itself.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeClientError::InvalidRequest`] for empty content,
    /// [`RuntimeClientError::RuntimeShutdown`] after shutdown, and
    /// [`RuntimeClientError::InvalidState`] for a mailbox admission
    /// failure.
    pub(crate) fn submit_inbound(
        &self,
        content: Vec<crate::message::types::UserContentBlock>,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        if content
            .iter()
            .any(|b| !matches!(b, crate::message::types::UserContentBlock::Text(_)))
        {
            return Err(RuntimeClientError::InvalidRequest {
                message: "user uploads require server-issued Session receipts".into(),
            });
        }
        self.submit_session_inbound(content)
    }

    pub(crate) fn edit_pending(
        &self,
        expected: &crate::durable::inbox::PendingInboundRef,
        text: &str,
    ) -> Result<crate::durable::inbox::PendingMutationOutcome, RuntimeClientError> {
        self.ensure_writable_runtime()?;
        let runtime = self.runtime.as_ref().expect("writable runtime");
        Ok(runtime
            .tool_runtime()
            .mailbox()
            .edit_pending(expected, text)
            .unwrap_or(crate::durable::inbox::PendingMutationOutcome::DurabilityUncertain))
    }

    pub(crate) fn remove_pending(
        &self,
        expected: &crate::durable::inbox::PendingInboundRef,
    ) -> Result<crate::durable::inbox::PendingMutationOutcome, RuntimeClientError> {
        self.ensure_writable_runtime()?;
        let runtime = self.runtime.as_ref().expect("writable runtime");
        Ok(runtime
            .tool_runtime()
            .mailbox()
            .remove_pending(expected)
            .unwrap_or(crate::durable::inbox::PendingMutationOutcome::DurabilityUncertain))
    }

    pub(crate) fn submit_session_inbound(
        &self,
        content: Vec<crate::message::types::UserContentBlock>,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        self.ensure_writable_runtime()?;
        let runtime = self
            .runtime
            .as_ref()
            .expect("a writable Runtime Client host has a runtime");
        let admission = runtime
            .submit_inbound(content)
            .map_err(|error| match error {
                InboundAdmissionError::Shutdown => RuntimeClientError::RuntimeShutdown,
                InboundAdmissionError::Inactive => RuntimeClientError::InvalidState {
                    message: "the conversation runtime is not activated".to_owned(),
                },
                InboundAdmissionError::EmptyContent => RuntimeClientError::InvalidRequest {
                    message: "inbound content must not be empty".to_owned(),
                },
                InboundAdmissionError::DurabilityFailed { message } => {
                    RuntimeClientError::InvalidState { message }
                }
                InboundAdmissionError::Mailbox(error) => RuntimeClientError::InvalidState {
                    message: error.to_string(),
                },
                error @ InboundAdmissionError::ModelUnavailable { .. } => {
                    RuntimeClientError::InvalidModelConfiguration {
                        message: error.to_string(),
                    }
                }
                // The activation guidance seal belongs to the child plane;
                // the human submit path never enters that class.
                error @ InboundAdmissionError::GuidanceSealed => RuntimeClientError::InvalidState {
                    message: error.to_string(),
                },
            })?;
        Ok(RuntimeClientResult::InboundAccepted {
            message_id: admission.message_id,
            inbound_sequence: admission.inbound_sequence,
        })
    }

    /// Requests cancellation through the native current-attempt owner.
    ///
    /// Selection and cancellation share the coordinator's admission/settlement
    /// lock. Observation availability and publication progress do not select
    /// the target or gate this control. The returned native identity denotes
    /// acceptance, while terminal settlement remains owned by the Agent Loop.
    ///
    /// # Errors
    /// Returns the native no-current-attempt or Goal-pause failure.
    pub(crate) fn cancel_current_attempt(&self) -> Result<RuntimeClientResult, RuntimeClientError> {
        self.ensure_writable_runtime()?;
        let result = self
            .runtime
            .as_ref()
            .expect("a writable Runtime Client host has a runtime")
            .cancel_active_attempt();
        #[cfg(test)]
        if let Some(hook) = self.cancellation_receipt_hook.lock().unwrap().take() {
            hook();
        }
        match result {
            Ok(attempt_id) => Ok(RuntimeClientResult::AttemptCancellationAccepted { attempt_id }),
            Err(CancelAttemptError::NoCurrentAttempt) => Err(RuntimeClientError::NoCurrentAttempt),
            // The attempt was cancelled, but the interrupt's durable Goal
            // pause did not commit (Issue #351). The client is told the
            // truth rather than shown a pause that does not exist.
            Err(CancelAttemptError::GoalPauseFailed { diagnostic }) => {
                Err(RuntimeClientError::InvalidState {
                    message: format!(
                        "the attempt was cancelled, but the Goal could not be durably paused: {diagnostic}"
                    ),
                })
            }
        }
    }

    /// Runs one manual idle context compaction to its terminal result and
    /// returns the exact native commit receipt for this invocation.
    pub(crate) async fn compact_context(
        &self,
        request_id: Option<crate::runtime::identity::ManualCompactionRequestId>,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        self.ensure_writable_runtime()?;
        let handle = self
            .runtime
            .as_ref()
            .expect("a writable Runtime Client host has a runtime")
            .admit_compaction(request_id)
            .map_err(Self::compaction_error)?;
        #[cfg(test)]
        let gate = self.manual_compaction_response_gate.lock().unwrap().take();
        Self::finish_compaction(
            handle,
            #[cfg(test)]
            gate,
        )
        .await
    }

    pub(crate) fn admit_compaction_transfer(
        &self,
        request_id: Option<crate::runtime::identity::ManualCompactionRequestId>,
    ) -> Result<
        futures_util::future::BoxFuture<'static, Result<RuntimeClientResult, RuntimeClientError>>,
        RuntimeClientError,
    > {
        self.ensure_writable_runtime()?;
        let handle = self
            .runtime
            .as_ref()
            .expect("writable runtime")
            .admit_compaction(request_id)
            .map_err(Self::compaction_error)?;
        // The transferred reply must not retain the host's Runtime or Store
        // allocation after native retirement. Only the exact receipt travels.
        Ok(Box::pin(Self::finish_compaction(
            handle,
            #[cfg(test)]
            self.manual_compaction_response_gate.lock().unwrap().take(),
        )))
    }

    #[cfg(test)]
    pub(crate) fn hold_compaction_response_for_test(
        &self,
    ) -> (
        tokio::sync::oneshot::Receiver<
            crate::runtime::conversation_runtime::ManualCompactionOutcome,
        >,
        tokio::sync::oneshot::Sender<()>,
    ) {
        let (committed, receipt) = tokio::sync::oneshot::channel();
        let (release, held) = tokio::sync::oneshot::channel();
        assert!(
            self.manual_compaction_response_gate
                .lock()
                .unwrap()
                .replace((committed, held))
                .is_none()
        );
        (receipt, release)
    }

    fn compaction_error(error: ManualCompactionError) -> RuntimeClientError {
        match error {
            ManualCompactionError::Inactive | ManualCompactionError::Busy => {
                RuntimeClientError::InvalidState {
                    message: error.to_string(),
                }
            }
            ManualCompactionError::Shutdown => RuntimeClientError::RuntimeShutdown,
            ManualCompactionError::ModelUnavailable { .. } => {
                RuntimeClientError::InvalidModelConfiguration {
                    message: error.to_string(),
                }
            }
            ManualCompactionError::DurabilityFailed { message }
            | ManualCompactionError::Durable { message } => {
                RuntimeClientError::InvalidState { message }
            }
            ManualCompactionError::Context(context)
                if matches!(
                    context.kind,
                    crate::context::ContextErrorKind::NoProgress
                        | crate::context::ContextErrorKind::CannotFit
                        | crate::context::ContextErrorKind::AuxiliaryBusy
                ) =>
            {
                RuntimeClientError::InvalidState {
                    message: context.message,
                }
            }
            ManualCompactionError::Context(context) => RuntimeClientError::RuntimeFailure {
                message: context.message,
            },
        }
    }

    async fn finish_compaction(
        handle: crate::runtime::conversation_runtime::ManualCompactionHandle,
        #[cfg(test)] gate: Option<super::ManualCompactionResponseGate>,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        let outcome = handle.wait().await.map_err(Self::compaction_error)?;
        #[cfg(test)]
        if let Some((committed, release)) = gate {
            let _ = committed.send(outcome.clone());
            let _ = release.await;
        }
        Ok(RuntimeClientResult::ContextCompacted {
            compaction: Some(crate::observation::head::RuntimeClientCompactionView {
                generation: outcome.generation,
                checkpoint_id: outcome.checkpoint_id,
                activation_id: outcome.activation_id,
                tokens_before: outcome.tokens_before,
                estimated_tokens_after: outcome.estimated_tokens_after,
            }),
        })
    }

    /// Accepts one typed native interaction response through the
    /// conversation-owned coordinator.
    ///
    /// # Errors
    ///
    /// Returns a typed Runtime Client error when the interaction is stale,
    /// already settled, or the response fails bounded validation.
    pub(crate) async fn respond_interaction(
        &self,
        interaction: &InteractionRef,
        response: InteractionResponse,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        self.ensure_writable_runtime()?;
        self.runtime
            .as_ref()
            .expect("a writable Runtime Client host has a runtime")
            .respond_interaction(interaction, response)
            .await
            .map(|()| RuntimeClientResult::InteractionResponseAccepted {
                interaction: interaction.clone(),
            })
            .map_err(|error| match error {
                RoutedInteractionError::NotPending { interaction } => {
                    RuntimeClientError::InteractionNotPending { interaction }
                }
                RoutedInteractionError::InvalidResponse { message } => {
                    RuntimeClientError::InteractionInvalidResponse { message }
                }
                RoutedInteractionError::AuditFailed { interaction } => {
                    RuntimeClientError::InteractionAuditFailed { interaction }
                }
            })
    }

    pub(crate) async fn cancel_interaction(
        &self,
        interaction: &InteractionRef,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        self.ensure_writable_runtime()?;
        self.runtime
            .as_ref()
            .expect("writable runtime")
            .control_interaction(
                interaction,
                crate::runtime::interaction::InteractionControl::Cancel,
            )
            .await
            .map(|()| RuntimeClientResult::InteractionResponseAccepted {
                interaction: interaction.clone(),
            })
            .map_err(|error| match error {
                RoutedInteractionError::NotPending { interaction } => {
                    RuntimeClientError::InteractionNotPending { interaction }
                }
                RoutedInteractionError::InvalidResponse { message } => {
                    RuntimeClientError::InteractionInvalidResponse { message }
                }
                RoutedInteractionError::AuditFailed { interaction } => {
                    RuntimeClientError::InteractionAuditFailed { interaction }
                }
            })
    }

    pub(crate) fn goal_control(
        &self,
        control: crate::goal::GoalControl,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        if matches!(control, crate::goal::GoalControl::Show) {
            return self.domain_page("goal", 0, 32);
        }
        let runtime = self
            .runtime
            .as_ref()
            .ok_or_else(|| RuntimeClientError::InvalidState {
                message: "No live Goal capability in historical inspection".to_owned(),
            })?;
        runtime
            .control_goal(control)
            .map(|view| RuntimeClientResult::GoalMutation {
                reference: view.current.map(|goal| goal.reference),
            })
            .map_err(|message| RuntimeClientError::InvalidRequest { message })
    }

    /// Reads the authoritative session model state through the folded
    /// projection, so the value always agrees with the snapshot read model.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeClientError::ProjectionExhausted`] when the
    /// observation stream is over.
    pub(crate) fn model_get(&self) -> Result<RuntimeClientResult, RuntimeClientError> {
        self.domain_page("model", 0, 32)
    }

    /// Replaces a standalone conversation's in-memory model configuration.
    /// Durable Session runtimes reject this control; their native Session owner
    /// must commit settings, retained binding, and runtime authority together.
    ///
    /// # Linearization
    ///
    /// The runtime resolves from an immutable capture off the coordinator
    /// lock, then fences and commits under the lock that also owns admission. An
    /// update therefore either linearizes before an admission (and that
    /// attempt observes it) or after it (and only later attempts observe
    /// it). There is no third possibility and no timing assumption.
    ///
    /// # Transactionality
    ///
    /// A rejected update changes nothing: the session keeps its previous
    /// configuration, no cursor is allocated, and no event is published.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeClientError::InvalidModelConfiguration`] when the
    /// configuration cannot be resolved against the catalog or cannot run
    /// under the session context policy, [`RuntimeClientError::InvalidState`]
    /// for a durable Session or while the runtime is not yet activated, and
    /// [`RuntimeClientError::ProjectionExhausted`] when the observation
    /// stream is over.
    pub(crate) fn model_set(
        &self,
        config: SessionModelConfig,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        self.ensure_writable_runtime()?;
        let state = self.lock_state();
        state.projection.snapshot_ref_checked()?;
        drop(state);
        self.runtime
            .as_ref()
            .expect("a writable Runtime Client host has a runtime")
            .model_set(config)
            .map_err(|error| match error {
                ModelUpdateError::SessionOwned => RuntimeClientError::InvalidState {
                    message: "Session model selection requires the native Session owner".to_owned(),
                },
                ModelUpdateError::Conflict => RuntimeClientError::ConfigurationAdoption {
                    rejection: crate::runtime::adoption::AdoptionError::Conflict,
                },
                ModelUpdateError::Inactive => RuntimeClientError::InvalidState {
                    message: "the conversation runtime is not activated".to_owned(),
                },
                ModelUpdateError::InvalidConfiguration(message) => {
                    RuntimeClientError::InvalidModelConfiguration { message }
                }
                ModelUpdateError::DurabilityFailed { message } => {
                    RuntimeClientError::InvalidState { message }
                }
            })?;
        Ok(RuntimeClientResult::ModelSet)
    }

    #[cfg(test)]
    pub(crate) fn job_status(
        &self,
        id: &ToolExecutionId,
    ) -> Result<super::super::snapshot::RuntimeClientJob, RuntimeClientError> {
        let snapshot = self
            .runtime
            .as_ref()
            .and_then(|r| r.background_status(id))
            .ok_or_else(|| RuntimeClientError::UnknownBackgroundExecution {
                execution_id: id.clone(),
            })?;
        Ok(background_view(&snapshot))
    }

    pub(crate) fn job_status_page(
        &self,
        id: &ToolExecutionId,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        if !self
            .runtime
            .as_ref()
            .is_some_and(|runtime| runtime.background_registry().contains_execution(id))
        {
            return Err(RuntimeClientError::UnknownBackgroundExecution {
                execution_id: id.clone(),
            });
        }
        self.domain_page(&format!("jobs.{id}"), 0, 32)
    }

    pub(crate) fn job_list(&self) -> Result<RuntimeClientResult, RuntimeClientError> {
        self.domain_page("jobs", 0, 32)
    }

    pub(crate) fn job_cancel_intent(
        &self,
        id: &ToolExecutionId,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        self.ensure_writable_runtime()?;
        let runtime = self
            .runtime
            .as_ref()
            .ok_or(RuntimeClientError::RuntimeShutdown)?;
        let job_id = runtime
            .background_cancel_intent(id)
            .map_err(|_| RuntimeClientError::RuntimeShutdown)?
            .ok_or_else(|| RuntimeClientError::UnknownBackgroundExecution {
                execution_id: id.clone(),
            })?;
        Ok(RuntimeClientResult::JobCancellationAccepted { job_id })
    }

    pub(crate) async fn job_wait(
        &self,
        id: &ToolExecutionId,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        let runtime = self.runtime.as_ref().ok_or_else(|| {
            RuntimeClientError::UnknownBackgroundExecution {
                execution_id: id.clone(),
            }
        })?;
        let snapshot = runtime
            .background_registry()
            .wait_until_terminal(id)
            .await
            .map_err(|error| match error {
                crate::tools::background::BackgroundWaitError::UnknownJob => {
                    RuntimeClientError::UnknownBackgroundExecution {
                        execution_id: id.clone(),
                    }
                }
                crate::tools::background::BackgroundWaitError::PublicationAbandoned => {
                    RuntimeClientError::JobPublicationAbandoned { job_id: id.clone() }
                }
            })?;
        Ok(RuntimeClientResult::JobOutcome {
            job_id: snapshot.execution_id.clone(),
            state: snapshot.state,
        })
    }

    /// Fences every semantic mutation from a durable inspection host. The
    /// read-only host owns no coordinator, mailbox, registry, or lifecycle
    /// handle, so an inspection client cannot accidentally become an
    /// execution owner through a control request.
    pub(super) fn ensure_writable_runtime(&self) -> Result<(), RuntimeClientError> {
        if self.read_only {
            return Err(RuntimeClientError::InvalidState {
                message: "conversation inspection is read-only".to_owned(),
            });
        }
        if self.runtime.is_none() {
            return Err(RuntimeClientError::InvalidState {
                message: "the conversation runtime is unavailable".to_owned(),
            });
        }
        Ok(())
    }

    /// Read the native capability projection.
    pub(crate) fn capability(&self) -> Result<RuntimeClientResult, RuntimeClientError> {
        self.domain_page("capabilities", 0, 32)
    }

    /// Reads the safe public model catalog through the conversation
    /// runtime's authoritative session model state.
    ///
    /// It never carries a credential value, an adapter, or a provider HTTP
    /// client.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeClientError::ProjectionExhausted`] when the
    /// observation stream is over.
    pub(crate) fn model_catalog(&self) -> Result<RuntimeClientResult, RuntimeClientError> {
        self.domain_page("model_catalog.models", 0, 32)
    }
}
