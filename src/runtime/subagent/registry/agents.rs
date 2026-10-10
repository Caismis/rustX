//! Durable child identity and activation-specific controls. All decisions use
//! the registry mutex shared by message admission, sealing and settlement.
use super::{
    AgentId, CancellationReason, CancellationSignal, ConversationId, GuidanceTicket, PoisonError,
    RegistryState, ResolvedSubagentSpec, SubagentExecutionProfile, SubagentId, SubagentLifecycle,
    SubagentObservation, SubagentRecord, SubagentRegistry, SubagentSnapshot, SubagentStartError,
    SubagentStartOutcome, SubagentStartSpec, SubagentSteerError, SubagentTerminalMode,
    SubagentTerminalState, SubagentWorkspaceResourceState, SubagentWorkspaceTerminalResource,
    WorkspaceUnresolvedRecord,
};
use serde::{Deserialize, Serialize};

/// Maximum number of durable Agent identities materialized by listing surfaces.
pub const MAX_AGENT_LIST_LIMIT: usize = 64;

/// Immutable Agent-lifetime executable authority. The entire value is captured
/// in private conversation storage, never serialized into Event Journal facts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DurableAgentAuthority {
    /// Immutable task label, independent of the named execution profile.
    pub title: String,
    pub resolved: ResolvedSubagentSpec,
    pub execution_policy: super::super::InheritedExecutionPolicy,
    pub approval_mode: crate::runtime::types::ApprovalMode,
}

pub(super) struct AgentRecord {
    pub created_sequence: u64,
    pub authority: DurableAgentAuthority,
    pub workspace: crate::runtime::workspace::AgentWorkspace,
    pub conversation_id: ConversationId,
    pub latest_activation: SubagentId,
    pub resuming: Option<ResumeReservation>,
}

impl AgentRecord {
    fn unavailable(&self, activation: &SubagentRecord) -> bool {
        // Native staging and cleanup may poison their workspace before the
        // registry installs their outcome. Publish that fact only with the
        // completed owner transition, never ahead of its observation.
        (self.resuming.is_none()
            && activation.lifecycle.is_terminal()
            && self.workspace.is_poisoned())
            || activation.publication_abandoned
            || (activation.lifecycle.is_terminal() && !activation.physical_settlement_proven)
            || self.resuming.as_ref().is_some_and(|reservation| {
                matches!(
                    *reservation.completion.borrow(),
                    AdmissionSettlement::Failed
                )
            })
    }

    pub(super) fn finish_resume(&mut self, activation_id: &SubagentId) {
        if self
            .resuming
            .as_ref()
            .is_some_and(|reservation| &reservation.activation_id == activation_id)
        {
            self.resuming = None;
        }
    }
}

#[derive(Clone, Copy)]
pub(super) enum AdmissionSettlement {
    Pending,
    Committed,
    RolledBack,
    Failed,
}
struct CapturedAgentActivation {
    id: Option<SubagentId>,
    admission: Option<tokio::sync::watch::Receiver<AdmissionSettlement>>,
}

pub(super) struct ResumeReservation {
    pub activation_id: SubagentId,
    /// Public identity is withheld until its durable allocation is installed.
    pub identity_persisted: bool,
    pub origin: super::super::AgentActivationOrigin,
    pub cancellation: CancellationSignal,
    pub completion: tokio::sync::watch::Sender<AdmissionSettlement>,
}

pub(super) struct ResumeIdentity {
    pub activation_id: SubagentId,
    pub workspace: crate::runtime::workspace::AgentWorkspace,
    pub workspace_access: std::sync::Mutex<Option<crate::runtime::workspace::AgentWorkspaceAccess>>,
    pub agent_id: AgentId,
    pub conversation_id: ConversationId,
    pub physical_owner:
        Option<std::sync::Arc<super::super::physical_recovery::ParentPhysicalLease>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AgentState {
    Admitting,
    Active,
    Stopping,
    Inactive,
    /// No autonomous activation transition can restore availability.
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AgentSnapshot {
    pub title: String,
    pub agent_id: AgentId,
    pub conversation_id: ConversationId,
    pub parent_agent_id: AgentId,
    pub agent: String,
    pub state: AgentState,
    /// The durable current/reserved activation. Admitting can temporarily have
    /// None while internal arbitration precedes durable identity allocation.
    pub current_activation: Option<SubagentId>,
    pub latest_activation: SubagentId,
    pub observation: SubagentObservation,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AgentMessageAccepted {
    pub agent_id: AgentId,
    pub activation_id: SubagentId,
    pub resumed: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AgentListing {
    pub agents: Vec<(AgentSnapshot, SubagentSnapshot)>,
    pub matched: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AgentWaitResult {
    pub agent_id: AgentId,
    pub activation_id: Option<SubagentId>,
    pub outcome: Option<SubagentSnapshot>,
}

#[derive(Debug, thiserror::Error)]
pub enum AgentControlError {
    #[error("unknown Agent in this conversation: {0}")]
    Unknown(AgentId),
    #[error("Agent is stopping or admitting an activation; retry after settlement")]
    Stopping,
    #[error("{0}")]
    Message(SubagentSteerError),
    #[error("{0}")]
    Start(SubagentStartError),
    #[error(
        "Agent is unavailable: physical settlement, canonical publication, or workspace authority requires explicit repair"
    )]
    Settlement,
    #[error("input delivery did not start; no input was delivered")]
    NotDelivered,
    #[error("child input acceptance was not acknowledged; delivery is unknown")]
    DeliveryUnknown,
    #[error("activation admission task failed: {0}")]
    Admission(String),
}

impl SubagentRegistry {
    pub(crate) async fn wait_for_agent_state(
        &self,
        id: &AgentId,
        predicate: impl Fn(AgentState) -> bool,
    ) -> Option<AgentSnapshot> {
        let mut changes = self.state_version.subscribe();
        loop {
            changes.borrow_and_update();
            let snapshot = self.agent_snapshot(id)?;
            if predicate(snapshot.state) {
                return Some(snapshot);
            }
            if changes.changed().await.is_err() {
                return None;
            }
        }
    }

    pub(crate) fn agent_artifact_root(&self, id: &AgentId) -> Option<std::path::PathBuf> {
        let agent = self.agent_snapshot(id)?;
        Some(
            super::super::child_conversation_store_path(
                self.config.spawn.product_root.root(),
                &self.config.spawn.session_id,
                &agent.conversation_id,
            )
            .parent()?
            .to_path_buf(),
        )
    }
    pub(crate) fn agent_workspace(
        &self,
        id: &AgentId,
    ) -> Option<super::super::super::workspace::WorkspaceSnapshot> {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let agent = state.agents.get(id)?;
        if agent.workspace.is_poisoned() {
            return None;
        }
        let activation = &state.records[*state.index.get(&agent.latest_activation)?];
        Some(activation.workspace.clone())
    }
    pub(crate) fn agent_inspection_path(&self, id: &AgentId) -> Option<std::path::PathBuf> {
        let agent = self.agent_snapshot(id)?;
        Some(super::super::child_conversation_inspection_socket_path(
            self.config.spawn.product_root.root(),
            &agent.conversation_id,
        ))
    }

    pub(super) fn agent_snapshot_locked(
        state: &RegistryState,
        id: &AgentId,
    ) -> Option<AgentSnapshot> {
        let agent = state.agents.get(id)?;
        let activation = &state.records[*state.index.get(&agent.latest_activation)?];
        let lifecycle = if agent.unavailable(activation) {
            AgentState::Unavailable
        } else if agent.resuming.is_some() {
            AgentState::Admitting
        } else {
            match activation.lifecycle {
                SubagentLifecycle::Running => AgentState::Active,
                SubagentLifecycle::Starting => AgentState::Admitting,
                lifecycle if lifecycle.is_terminal() && activation.physical_settlement_proven => {
                    AgentState::Inactive
                }
                _ => AgentState::Stopping,
            }
        };
        Some(AgentSnapshot {
            title: agent.authority.title.clone(),
            agent_id: id.clone(),
            conversation_id: agent.conversation_id.clone(),
            parent_agent_id: activation.parent_agent_id.clone(),
            agent: activation.agent.as_str().to_owned(),
            state: lifecycle,
            current_activation: agent
                .resuming
                .as_ref()
                .filter(|r| r.identity_persisted)
                .map(|r| r.activation_id.clone())
                .or_else(|| {
                    (!activation.lifecycle.is_terminal() || !activation.physical_settlement_proven)
                        .then(|| agent.latest_activation.clone())
                }),
            latest_activation: agent.latest_activation.clone(),
            observation: activation.observation.clone(),
        })
    }

    pub fn agent_snapshot(&self, id: &AgentId) -> Option<AgentSnapshot> {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        Self::agent_snapshot_locked(&state, id)
    }

    /// One authoritative read boundary for domain state and activation facts.
    pub fn agent_snapshot_with_activation(
        &self,
        id: &AgentId,
    ) -> Option<(AgentSnapshot, SubagentSnapshot)> {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let agent = Self::agent_snapshot_locked(&state, id)?;
        let activation = state.records[*state.index.get(&agent.latest_activation)?].snapshot();
        Some((agent, activation))
    }

    /// Fresh driver-owned execution evidence for this exact activation. Recovery
    /// records have no driver lease; neither Stopping nor a missing terminal is
    /// sufficient to invent live work. Read after durable folding, not before a
    /// potentially blocked store read.
    pub(crate) fn execution_started_at(
        &self,
        id: &SubagentId,
    ) -> Option<chrono::DateTime<chrono::Utc>> {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let record = &state.records[*state.index.get(id)?];
        record
            .execution
            .as_ref()?
            .running()
            .then_some(record.started_at)
    }

    /// Newest admitted durable identities first, with an honest bounded count.
    ///
    /// # Panics
    /// Panics if the registry violates its invariant that every Agent owns a
    /// latest activation record.
    pub fn list_agents(&self, limit: usize) -> AgentListing {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let mut identities: Vec<_> = state.agents.iter().collect();
        identities.sort_by(|(aid, a), (bid, b)| {
            b.created_sequence
                .cmp(&a.created_sequence)
                .then_with(|| aid.cmp(bid))
        });
        AgentListing {
            matched: identities.len(),
            agents: identities
                .into_iter()
                .take(limit.min(MAX_AGENT_LIST_LIMIT))
                .map(|(id, _)| {
                    let agent = Self::agent_snapshot_locked(&state, id)
                        .expect("Agent owns its latest activation");
                    let activation =
                        state.records[state.index[&agent.latest_activation]].snapshot();
                    (agent, activation)
                })
                .collect(),
        }
    }

    async fn wait_for_input_acceptance(
        &self,
        activation: &SubagentId,
    ) -> Result<(), AgentControlError> {
        let mut changes = self.state_version.subscribe();
        loop {
            changes.borrow_and_update();
            {
                let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
                let record = &state.records[*state
                    .index
                    .get(activation)
                    .ok_or(AgentControlError::Settlement)?];
                if record.delegate_delivery == super::DelegateDelivery::Accepted {
                    return Ok(());
                }
                if record.lifecycle.is_terminal() || record.publication_abandoned {
                    return Err(
                        if record.delegate_delivery == super::DelegateDelivery::Started {
                            AgentControlError::DeliveryUnknown
                        } else {
                            AgentControlError::NotDelivered
                        },
                    );
                }
            }
            changes
                .changed()
                .await
                .map_err(|_| AgentControlError::Settlement)?;
        }
    }

    /// Capture the exact admitted or reserved generation under the owner lock.
    fn capture_agent_activation(
        &self,
        id: &AgentId,
        interrupt: bool,
    ) -> Result<CapturedAgentActivation, AgentControlError> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let agent = state
            .agents
            .get(id)
            .ok_or_else(|| AgentControlError::Unknown(id.clone()))?;
        let activation = &state.records[state.index[&agent.latest_activation]];
        if agent.unavailable(activation) {
            return Err(AgentControlError::Settlement);
        }
        if let Some(reservation) = &agent.resuming {
            let activation_id = reservation.activation_id.clone();
            let admission = reservation.completion.subscribe();
            if interrupt {
                reservation.cancellation.cancel();
                // If canonical ownership is currently committing off-lock,
                // preserve this exact interruption reason when it publishes.
                if let Some((_, reason)) = state.ownership_committing.get_mut(&activation_id) {
                    reason.get_or_insert(CancellationReason::UserRequested);
                }
            }
            return Ok(CapturedAgentActivation {
                id: Some(activation_id),
                admission: Some(admission),
            });
        }
        let snapshot = Self::agent_snapshot_locked(&state, id).expect("Agent owns activation");
        Ok(CapturedAgentActivation {
            id: snapshot.current_activation,
            admission: None,
        })
    }

    async fn settle_captured_agent(
        &self,
        id: &AgentId,
        interrupt: bool,
    ) -> Result<AgentWaitResult, AgentControlError> {
        let CapturedAgentActivation {
            id: target,
            admission,
        } = self.capture_agent_activation(id, interrupt)?;
        if let Some(mut admission) = admission {
            loop {
                let completion = *admission.borrow_and_update();
                match completion {
                    AdmissionSettlement::Committed => break,
                    AdmissionSettlement::Failed => return Err(AgentControlError::Settlement),
                    AdmissionSettlement::RolledBack => {
                        return Ok(AgentWaitResult {
                            agent_id: id.clone(),
                            activation_id: target,
                            outcome: None,
                        });
                    }
                    AdmissionSettlement::Pending => {}
                }
                admission
                    .changed()
                    .await
                    .map_err(|_| AgentControlError::Settlement)?;
            }
        }
        let outcome = if let Some(target) = &target {
            if interrupt {
                let _ = self.cancel(target, CancellationReason::UserRequested);
            }
            self.wait_until_settled(target).await
        } else {
            None
        };
        if target.is_some() && !outcome.as_ref().is_some_and(SubagentSnapshot::is_settled) {
            return Err(AgentControlError::Settlement);
        }
        Ok(AgentWaitResult {
            agent_id: id.clone(),
            activation_id: target,
            outcome,
        })
    }

    /// Wait for exactly the reserved/current activation; Inactive is immediate.
    /// A captured reservation that rolls back returns its ID with no execution outcome.
    ///
    /// # Errors
    /// Unknown Agent or failed physical settlement/canonical publication.
    pub async fn wait_agent(&self, id: &AgentId) -> Result<AgentWaitResult, AgentControlError> {
        self.settle_captured_agent(id, false).await
    }

    /// Cancel the exact admission generation or committed activation, then settle it.
    ///
    /// # Errors
    /// Unknown Agent or failed physical settlement/canonical publication.
    pub async fn interrupt_agent(
        &self,
        id: &AgentId,
    ) -> Result<AgentWaitResult, AgentControlError> {
        self.settle_captured_agent(id, true).await
    }

    /// Selection and admission/reservation are one mutex transaction, never a
    /// status read followed by a separate steer/resume decision.
    ///
    /// # Errors
    /// Returns an error for invalid input, an unknown or stopping Agent, failed
    /// durable delivery, or failed activation admission.
    ///
    /// # Panics
    /// Panics if the internal invariant that durable Agents own an activation
    /// under the registry mutex is violated.
    #[allow(clippy::too_many_lines)] // Keep arbitration and its owned continuation together.
    pub async fn send_message(
        &self,
        id: &AgentId,
        message: &str,
        origin: super::AgentActivationOrigin,
        cancellation: CancellationSignal,
    ) -> Result<AgentMessageAccepted, AgentControlError> {
        self.send_message_with_attachments(id, message, &[], origin, cancellation)
            .await
    }
    /// Send Session-resolved files with guidance, or resume an inactive Agent.
    /// # Errors
    /// Rejects unknown Agents, transient settlement, invalid or empty input,
    /// and admission/publication failures without replaying the mutation.
    /// # Panics
    /// Panics if a durable Agent loses its activation under the registry mutex.
    #[allow(clippy::too_many_lines)] // One atomic arbitration and its owned continuation.
    pub async fn send_message_with_attachments(
        &self,
        id: &AgentId,
        message: &str,
        attachments: &[crate::message::content::UploadedFileRef],
        origin: super::AgentActivationOrigin,
        caller_cancellation: CancellationSignal,
    ) -> Result<AgentMessageAccepted, AgentControlError> {
        enum Decision {
            Deliver(
                SubagentId,
                u64,
                tokio::sync::oneshot::Receiver<super::super::ipc::ChildGuidanceOutcome>,
                GuidanceTicket,
            ),
            Resume(
                Box<SubagentStartSpec>,
                Box<ResumeIdentity>,
                CancellationSignal,
                tokio::sync::watch::Sender<AdmissionSettlement>,
                Option<crate::runtime::types::LifecycleAdmission>,
            ),
        }
        if caller_cancellation.is_cancelled() {
            return Err(AgentControlError::Start(SubagentStartError::Cancelled));
        }
        if attachments.is_empty() {
            Self::validate_guidance_message(message).map_err(AgentControlError::Message)?;
        } else if message.len() > super::MAX_TASK_BYTES
            || attachments.len()
                > crate::local_runtime::session::uploads::UPLOAD_POLICY.max_uploads_per_user_input
        {
            return Err(AgentControlError::Message(
                SubagentSteerError::InvalidMessage {
                    bytes: message.len(),
                },
            ));
        }
        let ownership = self
            .config
            .spawn
            .product_root
            .runtime_ownership_admission()
            .await
            .map_err(|error| AgentControlError::Admission(error.to_string()))?;
        let mut changes = self.state_version.subscribe();
        let decision = loop {
            changes.borrow_and_update();
            let decision = {
                let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
                if state.goal_idle_committing {
                    None
                } else {
                    if caller_cancellation.is_cancelled() {
                        return Err(AgentControlError::Start(SubagentStartError::Cancelled));
                    }
                    let agent = state
                        .agents
                        .get(id)
                        .ok_or_else(|| AgentControlError::Unknown(id.clone()))?;
                    let activation_id = agent.latest_activation.clone();
                    let activation = &state.records[state.index[&activation_id]];
                    if agent.unavailable(activation) {
                        return Err(AgentControlError::Settlement);
                    }
                    if agent.resuming.is_some() {
                        return Err(AgentControlError::Stopping);
                    }
                    let decision = match activation.lifecycle {
                        SubagentLifecycle::Running => {
                            let (sequence, answer, ticket) = self
                                .admit_guidance_locked(
                                    &mut state,
                                    &activation_id,
                                    message,
                                    attachments,
                                )
                                .map_err(|error| match error {
                                    SubagentSteerError::ControlLost => {
                                        AgentControlError::NotDelivered
                                    }
                                    error => AgentControlError::Message(error),
                                })?;
                            Decision::Deliver(activation_id, sequence, answer, ticket)
                        }
                        lifecycle
                            if lifecycle.is_terminal() && activation.physical_settlement_proven =>
                        {
                            let admission =
                                self.config.mailbox.begin_running_admission().map_err(|_| {
                                    AgentControlError::Start(
                                        SubagentStartError::ConversationInactive,
                                    )
                                })?;
                            let activation_id = SubagentId::for_conversation(
                                &self.config.conversation_id,
                                state.next_ordinal,
                            );
                            state.next_ordinal += 1;
                            let cancellation = caller_cancellation.child();
                            let agent =
                                state.agents.get_mut(id).expect("looked up under same lock");
                            let spec = SubagentStartSpec {
                                authority: agent.authority.clone(),
                                admission: super::ActivationAdmission {
                                    attachments: attachments.to_vec(),
                                    task: message.to_owned(),
                                    context: None,
                                    origin: origin.clone(),
                                    terminal: SubagentTerminalMode::Normal,
                                },
                            };
                            let (completion, _) =
                                tokio::sync::watch::channel(AdmissionSettlement::Pending);
                            agent.resuming = Some(ResumeReservation {
                                activation_id: activation_id.clone(),
                                identity_persisted: false,
                                origin: spec.admission.origin.clone(),
                                cancellation: cancellation.clone(),
                                completion: completion.clone(),
                            });
                            Decision::Resume(
                                Box::new(spec),
                                Box::new(ResumeIdentity {
                                    activation_id,
                                    workspace: agent.workspace.clone(),
                                    workspace_access: std::sync::Mutex::new(None),
                                    agent_id: id.clone(),
                                    conversation_id: agent.conversation_id.clone(),
                                    physical_owner: None,
                                }),
                                cancellation,
                                completion,
                                admission,
                            )
                        }
                        _ => return Err(AgentControlError::Stopping),
                    };
                    let index = state.index[&state.agents[id].latest_activation];
                    // In-memory reservation is the message/interruption arbitration boundary.
                    // Its owner task installs recoverable authority and commits Reserved off-lock.
                    if matches!(&decision, Decision::Resume(..)) {
                        super::publish_snapshot(&mut state, &self.state_version, index);
                    }
                    Some(decision)
                }
            };
            if let Some(decision) = decision {
                break decision;
            }
            tokio::select! {
                biased;
                () = caller_cancellation.cancelled() => {
                    return Err(AgentControlError::Start(SubagentStartError::Cancelled));
                }
                notification = changes.changed() => {
                    notification.map_err(|_| AgentControlError::Settlement)?;
                }
            }
        };
        match decision {
            Decision::Deliver(activation_id, sequence, answer, ticket) => {
                drop(ownership);
                let outcome = answer.await;
                self.record_child_decision(&activation_id, sequence, &outcome);
                #[cfg(test)]
                let acknowledgement_hook = {
                    let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
                    state.steer_acknowledgement_hook.clone()
                };
                #[cfg(test)]
                if let Some(hook) = acknowledgement_hook {
                    hook.park().await;
                }

                match outcome {
                    Ok(super::super::ipc::ChildGuidanceOutcome::Accepted) => {}
                    Ok(super::super::ipc::ChildGuidanceOutcome::Refused(refusal)) => {
                        return Err(AgentControlError::Message(
                            SubagentSteerError::ChildRefused {
                                detail: refusal.to_string(),
                            },
                        ));
                    }
                    Err(_) => {
                        return Err(
                            if ticket
                                .write_started
                                .load(std::sync::atomic::Ordering::Acquire)
                            {
                                AgentControlError::DeliveryUnknown
                            } else {
                                AgentControlError::NotDelivered
                            },
                        );
                    }
                }
                Ok(AgentMessageAccepted {
                    agent_id: id.clone(),
                    activation_id,
                    resumed: false,
                })
            }
            Decision::Resume(spec, mut identity, cancellation, completion, admission) => {
                // The owner retains staging/rollback even if the caller drops
                // its response future after the reservation committed.
                let registry = self.clone();
                let owner_task = tokio::spawn(async move {
                    let _admission = admission;
                    #[cfg(test)]
                    let mut test_gates = registry.state.lock().unwrap().resume_test_gates.take();
                    let mut reserved = false;
                    let mut result = async {
                        #[cfg(test)]
                        let authority_hook = registry.state.lock().unwrap().authority_install_hook.take();
                        #[cfg(test)]
                        if let Some(hook) = authority_hook { hook.wait(); }
                        // Authority exists before the first durable obligation. The same
                        // locked open-file description is inherited by the physical child.
                        identity.physical_owner = Some(std::sync::Arc::new(
                            registry.finish_physical_reservation(
                                &identity.conversation_id, &identity.activation_id,
                                super::super::physical_recovery::ParentPhysicalLease::reserve(
                                    &registry.config.spawn.product_root, &registry.config.spawn.session_id,
                                    &identity.conversation_id, &identity.activation_id,
                                ),
                            )?
                        ));
                        {
                            let mut state = registry.state.lock().unwrap_or_else(PoisonError::into_inner);
                            let agent = state.agents.get_mut(&identity.agent_id).expect("reserved Agent remains owned");
                            let reservation = agent.resuming.as_mut().expect("admission owner retains its reservation");
                            assert_eq!(reservation.activation_id, identity.activation_id);
                            reservation.identity_persisted = true;
                            let latest = agent.latest_activation.clone();
                            let index = state.index[&latest];
                            // This is the first public appearance of the reserved
                            // activation ID. Its consumed allocation is already durable.
                            super::publish_snapshot(&mut state, &registry.state_version, index);
                        }
                        // The complete recovered verification uses supervised Git.
                        // Each helper receives durable continuation authority before spawn,
                        // including this pre-Reserved physical lifetime.
                        let access = crate::runtime::workspace::with_physical_settlement_authority(
                            identity.physical_owner.clone(), identity.workspace.acquire(&cancellation),
                        ).await;
                        let _helper_proof = identity.physical_owner.as_ref().unwrap()
                            .prove_continuations()
                            .map_err(|error| SubagentStartError::Rollback { detail: error.to_string() })?
                            .ok_or_else(|| SubagentStartError::Rollback {
                                detail: "recovered workspace verification has unresolved physical helpers".into(),
                            })?;
                        if cancellation.is_cancelled() { return Err(SubagentStartError::Cancelled); }
                        *identity.workspace_access.get_mut().unwrap() = Some(
                            access.map_err(|detail| SubagentStartError::Workspace { detail })?
                        );
                        #[cfg(test)]
                        let reserved_hook = registry.state.lock().unwrap().reserved_commit_hook.take();
                        #[cfg(test)]
                        if let Some(hook) = reserved_hook { hook.wait(); }
                        let receipt = registry.config.mailbox.commit_agent_activation_admission(
                            &ownership,
                            super::super::admission_event(
                                &registry.config.conversation_id, &identity.agent_id,
                                &identity.activation_id, &spec.admission.origin,
                                crate::events::types::AgentActivationAdmissionPhase::Reserved,
                                registry.config.clock.now(),
                            ),
                        ).map_err(|error| SubagentStartError::Durability { detail: error.to_string() })?;
                        reserved = true;
                        {
                            let mut state = registry.state.lock().unwrap_or_else(PoisonError::into_inner);
                            let agent = &state.agents[&identity.agent_id];
                            assert!(agent.resuming.as_ref().is_some_and(|reservation| reservation.activation_id == identity.activation_id));
                            let index = state.index[&agent.latest_activation];
                            super::publish_committed_snapshot(&mut state, &registry.state_version, index, receipt.sequence);
                        }
                        drop(ownership);
                        let prepared = registry
                            .prepare_inner(&spec, &cancellation, &mut None, Some(&identity))
                            .await?;
                        #[cfg(test)]
                        if let Some(gates) = test_gates.as_mut() {
                            // Staging owns real physical resources, but ownership has not committed.
                            let (unused, _) = tokio::sync::oneshot::channel();
                            let _ = std::mem::replace(&mut gates.staged, unused).send(());
                            let _ = (&mut gates.release_staged).await;
                        }
                        registry.commit(prepared, &cancellation).await
                    }
                    .await;
                    #[cfg(test)]
                    if let Some(owner) = &identity.physical_owner {
                        let hook = registry.state.lock().unwrap().resume_cleanup_hook.take();
                        if let Some(hook) = hook {
                            hook(owner);
                        }
                    }
                    // A pre-prepare cancellation may leave the verified access
                    // unconsumed. Release it before publishing Inactive so a
                    // later activation observes the same completed owner cut.
                    drop(identity.workspace_access.get_mut().unwrap().take());
                    if !reserved && result.is_err() && identity.physical_owner.is_some() {
                        registry.retain_unreserved_allocation(
                            &identity.conversation_id,
                            &identity.activation_id,
                        );
                    }
                    let mut rollback_sequence = None;
                    if reserved && !matches!(&result, Ok(SubagentStartOutcome::Accepted(_))) {
                        let mut physical_settlement_proven = match &result {
                            Ok(SubagentStartOutcome::RolledBack) => true,
                            Err(error) => error.rollback_is_proven(),
                            Ok(SubagentStartOutcome::Accepted(_)) => {
                                unreachable!("accepted activation never rolls back admission")
                            }
                        };
                        if physical_settlement_proven
                            && let Some(owner) = &identity.physical_owner
                            && let Err(error) = owner.publish_quiescent()
                        {
                            physical_settlement_proven = false;
                            result = Err(SubagentStartError::Rollback {
                                detail: format!(
                                    "rollback recovery proof publication failed: {error}"
                                ),
                            });
                        }
                        let publication = async {
                            let ownership = registry.config.spawn.product_root.runtime_ownership_admission().await
                                .map_err(|error| error.to_string())?;
                            registry.config.mailbox.commit_agent_activation_admission(&ownership, super::super::admission_event(
                                &registry.config.conversation_id, &identity.agent_id, &identity.activation_id,
                                &spec.admission.origin,
                                crate::events::types::AgentActivationAdmissionPhase::RolledBack { physical_settlement_proven },
                                registry.config.clock.now(),
                            )).map_err(|error| error.to_string())
                        }.await;
                        match publication {
                            Ok(receipt) => rollback_sequence = Some(receipt.sequence),
                            Err(detail) => {
                                result = Err(SubagentStartError::Rollback {
                                    detail: format!(
                                        "admission rollback proof could not be committed: {detail}"
                                    ),
                                });
                            }
                        }
                    }
                    {
                        let mut state = registry
                            .state
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner);
                        let unreserved = state
                            .recovery_unreserved
                            .contains_key(&identity.activation_id);
                        let agent = state
                            .agents
                            .get_mut(&identity.agent_id)
                            .expect("durable Agent survives activation");
                        if unreserved {
                            // Recovery now owns this consumed identity. Its workspace
                            // fence keeps the Agent unavailable without inventing Reserved.
                            agent.finish_resume(&identity.activation_id);
                        } else if matches!(&result, Err(SubagentStartError::Rollback { .. })) {
                            agent.workspace.poison();
                            if let Some(reservation) = &agent.resuming {
                                reservation
                                    .completion
                                    .send_replace(AdmissionSettlement::Failed);
                            }
                        } else {
                            agent.finish_resume(&identity.activation_id);
                        }
                        let index =
                            state.index[&state.agents[&identity.agent_id].latest_activation];
                        if let Some(sequence) = rollback_sequence {
                            super::publish_committed_snapshot(
                                &mut state,
                                &registry.state_version,
                                index,
                                sequence,
                            );
                        } else {
                            super::publish_snapshot(&mut state, &registry.state_version, index);
                        }
                    }
                    let had_physical_owner = identity.physical_owner.is_some();
                    // No executable handle survives the transfer. Reconciliation must
                    // acquire native proof; dropping the parent alone is not settlement.
                    if !reserved && !matches!(&result, Ok(SubagentStartOutcome::Accepted(_))) {
                        drop(identity.physical_owner.take());
                        registry.reconcile_recovered_settlements();
                        registry.start_recovery_reconciliation();
                        registry
                            .wait_recovery_settlement_for(&identity.activation_id)
                            .await;
                    }
                    let recovery_pending = registry
                        .state
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .recovery_pending
                        .contains(&identity.activation_id);
                    let completion_outcome = match &result {
                        _ if recovery_pending || !had_physical_owner => AdmissionSettlement::Failed,
                        Err(SubagentStartError::Rollback { .. }) => AdmissionSettlement::Failed,
                        Ok(SubagentStartOutcome::Accepted(_)) => AdmissionSettlement::Committed,
                        _ => AdmissionSettlement::RolledBack,
                    };
                    completion.send_replace(completion_outcome);
                    #[cfg(test)]
                    if let Some(gates) = test_gates {
                        // Durable rollback and owner publication precede release of LifecycleAdmission.
                        let _ = gates.published.send(());
                        let _ = gates.release_published.await;
                    }
                    match result.map_err(|error| match error {
                        SubagentStartError::Rollback { .. } => AgentControlError::Settlement,
                        error => AgentControlError::Start(error),
                    })? {
                        SubagentStartOutcome::Accepted(accepted) => {
                            registry
                                .wait_for_input_acceptance(&accepted.subagent_id)
                                .await?;
                            Ok(AgentMessageAccepted {
                                agent_id: identity.agent_id,
                                activation_id: accepted.subagent_id,
                                resumed: true,
                            })
                        }
                        SubagentStartOutcome::RolledBack => {
                            Err(AgentControlError::Start(SubagentStartError::Cancelled))
                        }
                    }
                });
                #[cfg(test)]
                {
                    self.state.lock().unwrap().resume_owner_abort = Some(owner_task.abort_handle());
                }
                owner_task
                    .await
                    .map_err(|error| AgentControlError::Admission(error.to_string()))?
            }
        }
    }
}

impl SubagentRegistry {
    /// Rebuild durable identity/activation relationships from committed facts.
    /// Startup reconciliation has already terminalized orphaned activations.
    /// No current settings or named-Agent definitions participate.
    #[allow(clippy::too_many_lines)] // one ordered fold of durable ownership/resource facts
    pub(crate) fn restore_agents(
        &self,
        store: &dyn crate::durable::ConversationStore,
    ) -> Result<(), crate::durable::ConversationStoreError> {
        use crate::durable::ConversationStoreError;
        use crate::events::types::RuntimeEvent;
        let mut pending_admissions = std::collections::BTreeMap::new();
        let mut unsettled = std::collections::BTreeSet::new();
        let mut cursor = None;
        let mut events = Vec::new();
        let mut authorities = std::collections::BTreeMap::new();
        loop {
            let page = store.read_events(cursor, 256)?;
            if page.events.is_empty() {
                break;
            }
            cursor = page.next_sequence;
            for envelope in page.events {
                if let RuntimeEvent::SubagentOwnershipCommitted {
                    admitted_authority: Some(agent_id),
                    ..
                } = &envelope.event
                {
                    authorities.insert(agent_id.clone(), store.load_agent_authority(agent_id)?);
                }
                events.push(envelope);
            }
        }
        let consumed = super::super::physical_recovery::consumed_session_allocations(
            &self.config.spawn.product_root,
            &self.config.spawn.session_id,
        )
        .map_err(|error| ConversationStoreError::InvalidReference(error.to_string()))?;
        let next_ordinal = consumed
            .iter()
            .map(|(activation, _)| activation)
            .chain(events.iter().filter_map(|envelope| match &envelope.event {
                RuntimeEvent::SubagentOwnershipCommitted { subagent_id, .. } => Some(subagent_id),
                RuntimeEvent::AgentActivationAdmission { activation_id, .. } => Some(activation_id),
                _ => None,
            }))
            .filter_map(|id| id.conversation_ordinal(&self.config.conversation_id))
            .map(|ordinal| ordinal.saturating_add(1))
            .max()
            .unwrap_or(1);
        // Physical namespaces retain the Conversation even before the first
        // logical Agent ownership exists. No absent record discharges a lease.
        let recorded: std::collections::BTreeSet<_> = events
            .iter()
            .filter_map(|envelope| match &envelope.event {
                RuntimeEvent::SubagentOwnershipCommitted { subagent_id, .. } => {
                    Some(subagent_id.clone())
                }
                RuntimeEvent::AgentActivationAdmission { activation_id, .. } => {
                    Some(activation_id.clone())
                }
                _ => None,
            })
            .collect();
        // The I/O phase above finishes before this startup-only replay cut.
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.next_ordinal = state.next_ordinal.max(next_ordinal);
        for envelope in events {
            match envelope.event {
                RuntimeEvent::AgentActivationAdmission {
                    agent_id,
                    activation_id,
                    origin,
                    phase,
                } => match phase {
                    crate::events::types::AgentActivationAdmissionPhase::Reserved => {
                        pending_admissions.insert(activation_id, (agent_id, origin));
                    }
                    crate::events::types::AgentActivationAdmissionPhase::RolledBack {
                        physical_settlement_proven: true,
                    } => {
                        pending_admissions.remove(&activation_id);
                    }
                    crate::events::types::AgentActivationAdmissionPhase::RolledBack {
                        physical_settlement_proven: false,
                    } => {}
                },
                RuntimeEvent::SubagentOwnershipCommitted {
                    parent_agent_id,
                    subagent_id,
                    child_agent_id,
                    child_conversation_id,
                    origin,
                    admitted_authority,
                    workspace,
                    ownership: crate::events::types::SubagentOwnershipKind::Normal,
                    ..
                } => {
                    if let Some(existing) = state.agents.get(&child_agent_id) {
                        let prior = &state.records[state.index[&existing.latest_activation]];
                        if admitted_authority.is_some()
                            || existing.conversation_id != child_conversation_id
                            || prior.parent_agent_id != parent_agent_id
                            || prior.workspace != workspace
                        {
                            return Err(ConversationStoreError::InvalidReference(format!(
                                "Agent {child_agent_id} changed admitted authority or identity"
                            )));
                        }
                    } else if admitted_authority.is_none() {
                        return Err(ConversationStoreError::InvalidReference(format!(
                            "Agent {child_agent_id} has no initial frozen authority"
                        )));
                    }
                    pending_admissions.remove(&subagent_id);
                    unsettled.insert(subagent_id.clone());
                    if let Some(reference) = admitted_authority {
                        if reference != child_agent_id {
                            return Err(ConversationStoreError::InvalidReference(
                                "Agent authority reference names a different identity".to_owned(),
                            ));
                        }
                        let authority = authorities
                            .remove(&child_agent_id)
                            .expect("referenced Agent authority loaded before registry replay");
                        let scope = crate::runtime::workspace::AgentWorkspace::recovered(
                            self.config.workspace.clone(),
                            subagent_id.clone(),
                            authority.resolved.workspace_policy,
                            workspace.clone(),
                            false,
                        );
                        state.agents.insert(
                            child_agent_id.clone(),
                            AgentRecord {
                                created_sequence: envelope.sequence,
                                authority,
                                workspace: scope,
                                conversation_id: child_conversation_id.clone(),
                                latest_activation: subagent_id.clone(),
                                resuming: None,
                            },
                        );
                    }
                    let agent = state
                        .agents
                        .get_mut(&child_agent_id)
                        .expect("initial authority validated above");
                    agent.latest_activation = subagent_id.clone();
                    let spec = agent.authority.clone();
                    if !state.index.contains_key(&subagent_id) {
                        let index = state.records.len();
                        state.index.insert(subagent_id.clone(), index);
                        let evidence = crate::runtime::recovery::SubagentEvidence {
                            subagent_id,
                            child_agent_id,
                            child_conversation_id,
                            origin,
                            agent: spec.resolved.agent.as_str().to_owned(),
                            definition_digest: spec.resolved.definition_digest.as_str().to_owned(),
                            profile_digest: spec.resolved.profile_digest().as_str().to_owned(),
                            ownership: crate::events::types::SubagentOwnershipKind::Normal,
                            workspace,
                            started_at: envelope.timestamp,
                        };
                        let mut record = SubagentRecord::recovered(parent_agent_id, &evidence)?;
                        record.profile =
                            Some(SubagentExecutionProfile::from_frozen(&spec.resolved.model));
                        state.records.push(record);
                    }
                }
                RuntimeEvent::SubagentOwnershipCommitted {
                    parent_agent_id,
                    subagent_id,
                    child_agent_id,
                    child_conversation_id,
                    origin,
                    agent,
                    definition_digest,
                    profile_digest,
                    admitted_authority,
                    workspace,
                    ownership: crate::events::types::SubagentOwnershipKind::Workflow,
                } => {
                    if admitted_authority.is_some() {
                        return Err(ConversationStoreError::InvalidReference(
                            "a finite Workflow child cannot own durable Agent authority".into(),
                        ));
                    }
                    unsettled.insert(subagent_id.clone());
                    if !state.index.contains_key(&subagent_id) {
                        let index = state.records.len();
                        state.index.insert(subagent_id.clone(), index);
                        let evidence = crate::runtime::recovery::SubagentEvidence {
                            subagent_id,
                            child_agent_id,
                            child_conversation_id,
                            origin,
                            agent,
                            definition_digest,
                            profile_digest,
                            ownership: crate::events::types::SubagentOwnershipKind::Workflow,
                            workspace,
                            started_at: envelope.timestamp,
                        };
                        state
                            .records
                            .push(SubagentRecord::recovered(parent_agent_id, &evidence)?);
                    }
                }
                RuntimeEvent::SubagentTerminalPublished {
                    subagent_id,
                    state: terminal,
                    workspace_resource,
                    physical_settlement_proven,
                    ..
                }
                | RuntimeEvent::SubagentTerminalSettled {
                    subagent_id,
                    state: terminal,
                    workspace_resource,
                    physical_settlement_proven,
                    ..
                } => {
                    unsettled.remove(&subagent_id);
                    if let Some(&index) = state.index.get(&subagent_id) {
                        let record = &mut state.records[index];
                        let child_agent_id = record.child_agent_id.clone();
                        match workspace_resource {
                            SubagentWorkspaceTerminalResource::PreservedUnresolved {
                                reason,
                                detail,
                            } => {
                                record.workspace_resource_state =
                                    SubagentWorkspaceResourceState::PreservedUnresolved;
                                record.workspace_unresolved =
                                    Some(WorkspaceUnresolvedRecord { reason, detail });
                                if let Some(agent) = state.agents.get(&child_agent_id) {
                                    agent.workspace.poison();
                                }
                            }
                            SubagentWorkspaceTerminalResource::Retained { handoff } => {
                                record.workspace_resource_state =
                                    SubagentWorkspaceResourceState::Retained;
                                record.handoff = Some(handoff);
                            }
                            SubagentWorkspaceTerminalResource::None => {}
                        }
                        state.records[index].physical_settlement_proven =
                            physical_settlement_proven;
                        state.records[index].lifecycle = match terminal {
                            SubagentTerminalState::Succeeded => SubagentLifecycle::Succeeded,
                            SubagentTerminalState::Failed => SubagentLifecycle::Failed,
                            SubagentTerminalState::Cancelled => SubagentLifecycle::Cancelled,
                            SubagentTerminalState::Interrupted => SubagentLifecycle::Interrupted,
                        };
                        if !physical_settlement_proven {
                            if let Some(agent) = state.agents.get(&child_agent_id) {
                                agent.workspace.await_recovered_physical_proof();
                            }
                            state.recovery_pending.insert(subagent_id);
                        }
                    }
                }
                RuntimeEvent::SubagentPhysicalSettlementProven { subagent_id, .. } => {
                    state.recovery_pending.remove(&subagent_id);
                    if let Some(&index) = state.index.get(&subagent_id) {
                        state.records[index].physical_settlement_proven = true;
                        if let Some(agent) = state.agents.get(&state.records[index].child_agent_id)
                        {
                            agent.workspace.prove_recovered_physical_settlement();
                        }
                    }
                }
                RuntimeEvent::SubagentWorkspaceDisposalStarted {
                    subagent_id,
                    workspace_handoff,
                } => {
                    if let Some(&index) = state.index.get(&subagent_id) {
                        let record = &mut state.records[index];
                        record.workspace_resource_state =
                            SubagentWorkspaceResourceState::DisposalInProgress;
                        // Disposal intent transfers the retained handoff
                        // into the exact private cleanup authority. It is
                        // no longer an available public handoff, including
                        // when replay follows the historical terminal.
                        record.handoff = None;
                        record.workspace_unresolved = None;
                        record.workspace_disposal = Some(super::WorkspaceDisposalRecord {
                            handoff: workspace_handoff,
                            phase: super::WorkspaceDisposalPhase::Authorized,
                        });
                        let child = record.child_agent_id.clone();
                        if let Some(agent) = state.agents.get(&child) {
                            agent.workspace.poison();
                        }
                    }
                }
                RuntimeEvent::SubagentWorkspaceDisposalSettled {
                    subagent_id,
                    workspace_handoff,
                    settlement,
                } => {
                    if let Some(&index) = state.index.get(&subagent_id) {
                        let record = &mut state.records[index];
                        match settlement {
                                crate::events::types::SubagentWorkspaceDisposalSettlement::WorktreeRemoved => {
                                    record.workspace_resource_state = SubagentWorkspaceResourceState::WorktreeRemoved;
                                    record.handoff = None;
                                    record.workspace_unresolved = None;
                                    record.workspace_disposal = Some(super::WorkspaceDisposalRecord {
                                        handoff: workspace_handoff,
                                        phase: super::WorkspaceDisposalPhase::WorktreeRemoved,
                                    });
                                }
                                crate::events::types::SubagentWorkspaceDisposalSettlement::Disposed => {
                                    record.workspace_resource_state = SubagentWorkspaceResourceState::Disposed;
                                    record.workspace_disposal = None;
                                    record.workspace_unresolved = None;
                                    record.handoff = None;
                                }
                            }
                        let child = record.child_agent_id.clone();
                        if let Some(agent) = state.agents.get(&child) {
                            agent.workspace.poison();
                        }
                    }
                }
                _ => {}
            }
        }
        for (activation, conversation) in consumed {
            if activation
                .conversation_ordinal(&self.config.conversation_id)
                .is_none()
                || recorded.contains(&activation)
            {
                continue;
            }
            if let Some(agent) = state
                .agents
                .values()
                .find(|agent| agent.conversation_id == conversation)
            {
                agent.workspace.await_recovered_physical_proof();
            }
            if let Some(previous) = state
                .recovery_unreserved
                .insert(activation.clone(), conversation.clone())
                && previous != conversation
            {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "physical allocation {activation} names multiple child Conversations"
                )));
            }
            state.recovery_pending.insert(activation);
        }
        for (activation_id, (agent_id, origin)) in pending_admissions {
            if let Some(agent) = state.agents.get_mut(&agent_id) {
                agent.workspace.await_recovered_physical_proof();
                let (completion, _) = tokio::sync::watch::channel(AdmissionSettlement::Failed);
                agent.resuming = Some(ResumeReservation {
                    activation_id: activation_id.clone(),
                    identity_persisted: true,
                    origin,
                    cancellation: CancellationSignal::new(),
                    completion,
                });
                state.recovery_pending.insert(activation_id);
            }
        }
        for activation in unsettled {
            if let Some(&index) = state.index.get(&activation)
                && let Some(agent) = state.agents.get(&state.records[index].child_agent_id)
            {
                // An orphan without reconciliation evidence is not proof of
                // physical containment. Never silently grant a fresh lease.
                agent.workspace.poison();
            }
        }
        drop(state);
        self.reconcile_recovered_settlements();
        self.start_recovery_reconciliation();
        Ok(())
    }
}
