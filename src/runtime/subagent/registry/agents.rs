//! Durable child identity and activation-specific controls. All decisions use
//! the registry mutex shared by message admission, sealing and settlement.
use super::{
    AgentId, CancellationReason, CancellationSignal, ConversationId, GuidanceTicket,
    NotificationState, PoisonError, RegistryState, ResolvedSubagentSpec, SubagentExecutionProfile,
    SubagentId, SubagentLifecycle, SubagentObservation, SubagentRecord, SubagentRegistry,
    SubagentSnapshot, SubagentStartError, SubagentStartOutcome, SubagentStartSpec,
    SubagentSteerError, SubagentTerminalMode, SubagentTerminalState,
    SubagentWorkspaceResourceState, SubagentWorkspaceTerminalResource, WorkspaceUnresolvedRecord,
};
use serde::{Deserialize, Serialize};

/// Maximum number of durable Agent identities materialized by listing surfaces.
pub const MAX_AGENT_LIST_LIMIT: usize = 64;

/// Immutable Agent-lifetime executable authority. The entire value is captured
/// in private conversation storage, never serialized into Event Journal facts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DurableAgentAuthority {
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
    pub origin: super::super::AgentActivationOrigin,
    pub cancellation: CancellationSignal,
    pub completion: tokio::sync::watch::Sender<AdmissionSettlement>,
}

pub(super) struct ResumeIdentity {
    pub activation_id: SubagentId,
    pub workspace: crate::runtime::workspace::AgentWorkspace,
    pub agent_id: AgentId,
    pub conversation_id: ConversationId,
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
    pub agent_id: AgentId,
    pub conversation_id: ConversationId,
    pub parent_agent_id: AgentId,
    pub agent: String,
    pub state: AgentState,
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
    #[error("activation admission task failed: {0}")]
    Admission(String),
}

impl SubagentRegistry {
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
            agent_id: id.clone(),
            conversation_id: agent.conversation_id.clone(),
            parent_agent_id: activation.parent_agent_id.clone(),
            agent: activation.agent.as_str().to_owned(),
            state: lifecycle,
            current_activation: agent
                .resuming
                .as_ref()
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
                if record.input_accepted {
                    return Ok(());
                }
                if record.lifecycle.is_terminal() || record.publication_abandoned {
                    return Err(AgentControlError::Admission(
                        "child input acceptance was not acknowledged; delivery may be unknown"
                            .into(),
                    ));
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
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let agent = state
            .agents
            .get(id)
            .ok_or_else(|| AgentControlError::Unknown(id.clone()))?;
        let activation = &state.records[state.index[&agent.latest_activation]];
        if agent.unavailable(activation) {
            return Err(AgentControlError::Settlement);
        }
        if let Some(reservation) = &agent.resuming {
            if interrupt {
                reservation.cancellation.cancel();
            }
            return Ok(CapturedAgentActivation {
                id: Some(reservation.activation_id.clone()),
                admission: Some(reservation.completion.subscribe()),
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
        Self::validate_guidance_message(message).map_err(AgentControlError::Message)?;
        let ownership = self
            .config
            .spawn
            .product_root
            .runtime_ownership_admission()
            .await
            .map_err(|error| AgentControlError::Admission(error.to_string()))?;
        let mut admission_sequence = None;
        let decision = {
            let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
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
                        .admit_guidance_locked(&mut state, &activation_id, message)
                        .map_err(AgentControlError::Message)?;
                    Decision::Deliver(activation_id, sequence, answer, ticket)
                }
                lifecycle if lifecycle.is_terminal() && activation.physical_settlement_proven => {
                    let admission =
                        self.config.mailbox.begin_running_admission().map_err(|_| {
                            AgentControlError::Start(SubagentStartError::ConversationInactive)
                        })?;
                    let activation_id = SubagentId::for_conversation(
                        &self.config.conversation_id,
                        state.next_ordinal,
                    );
                    state.next_ordinal += 1;
                    let cancellation = caller_cancellation.child();
                    let agent = state.agents.get_mut(id).expect("looked up under same lock");
                    let spec = SubagentStartSpec {
                        authority: agent.authority.clone(),
                        admission: super::ActivationAdmission {
                            task: message.to_owned(),
                            context: None,
                            origin,
                            terminal: SubagentTerminalMode::Normal,
                        },
                    };
                    let receipt = self
                        .config
                        .mailbox
                        .commit_agent_activation_admission(
                            &ownership,
                            super::super::admission_event(
                                &self.config.conversation_id,
                                id,
                                &activation_id,
                                &spec.admission.origin,
                                crate::events::types::AgentActivationAdmissionPhase::Reserved,
                                self.config.clock.now(),
                            ),
                        )
                        .map_err(|error| AgentControlError::Admission(error.to_string()))?;
                    admission_sequence = Some(receipt.sequence);
                    let (completion, _) = tokio::sync::watch::channel(AdmissionSettlement::Pending);
                    agent.resuming = Some(ResumeReservation {
                        activation_id: activation_id.clone(),
                        origin: spec.admission.origin.clone(),
                        cancellation: cancellation.clone(),
                        completion: completion.clone(),
                    });
                    Decision::Resume(
                        Box::new(spec),
                        Box::new(ResumeIdentity {
                            activation_id,
                            workspace: agent.workspace.clone(),
                            agent_id: id.clone(),
                            conversation_id: agent.conversation_id.clone(),
                        }),
                        cancellation,
                        completion,
                        admission,
                    )
                }
                _ => return Err(AgentControlError::Stopping),
            };
            let index = state.index[&state.agents[id].latest_activation];
            if let Some(sequence) = admission_sequence {
                super::publish_committed_snapshot(&mut state, &self.state_version, index, sequence);
            } else {
                super::publish_snapshot(&mut state, &self.state_version, index);
            }
            decision
        };
        drop(ownership);
        match decision {
            Decision::Deliver(activation_id, sequence, answer, _ticket) => {
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
                        return Err(AgentControlError::Message(
                            SubagentSteerError::ChildRefused {
                                detail: "the child settled before the guidance was accepted"
                                    .to_owned(),
                            },
                        ));
                    }
                }
                Ok(AgentMessageAccepted {
                    agent_id: id.clone(),
                    activation_id,
                    resumed: false,
                })
            }
            Decision::Resume(spec, identity, cancellation, completion, admission) => {
                // The owner retains staging/rollback even if the caller drops
                // its response future after the reservation committed.
                let registry = self.clone();
                tokio::spawn(async move {
                    let _admission = admission;
                    #[cfg(test)]
                    let mut test_gates = registry.state.lock().unwrap().resume_test_gates.take();
                    let mut result = async {
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
                    let mut rollback_sequence = None;
                    if !matches!(&result, Ok(SubagentStartOutcome::Accepted(_))) {
                        let physical_settlement_proven = !matches!(&result, Err(SubagentStartError::Rollback { .. }));
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
                            Err(detail) => result = Err(SubagentStartError::Rollback { detail: format!("admission rollback proof could not be committed: {detail}") }),
                        }
                    }
                    {
                        let mut state = registry
                            .state
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner);
                        let agent = state
                            .agents
                            .get_mut(&identity.agent_id)
                            .expect("durable Agent survives activation");
                        if matches!(&result, Err(SubagentStartError::Rollback { .. })) {
                            agent.workspace.poison();
                            if let Some(reservation) = &agent.resuming {
                                reservation
                                    .completion
                                    .send_replace(AdmissionSettlement::Failed);
                            }
                        } else {
                            agent.finish_resume(&identity.activation_id);
                        }
                        let index = state.index[&state.agents[&identity.agent_id].latest_activation];
                        if let Some(sequence) = rollback_sequence {
                            super::publish_committed_snapshot(&mut state, &registry.state_version, index, sequence);
                        } else {
                            super::publish_snapshot(&mut state, &registry.state_version, index);
                        }
                    }
                    let completion_outcome = match &result {
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
                })
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
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        loop {
            let page = store.read_events(cursor, 256)?;
            if page.events.is_empty() {
                break;
            }
            cursor = page.next_sequence;
            for envelope in page.events {
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
                                    "Agent authority reference names a different identity"
                                        .to_owned(),
                                ));
                            }
                            let authority = store.load_agent_authority(&child_agent_id)?;
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
                            state.records.push(SubagentRecord {
                                physical_settlement_proven: false,
                                input_accepted: true,
                                parent_agent_id,
                                subagent_id,
                                child_agent_id,
                                child_conversation_id,
                                origin,
                                agent: spec.resolved.agent.clone(),
                                definition_digest: spec.resolved.definition_digest.clone(),
                                profile_digest: spec.resolved.profile_digest(),
                                ownership: crate::events::types::SubagentOwnershipKind::Normal,
                                terminal: None,
                                workspace,
                                handoff: None,
                                workspace_resource_state: SubagentWorkspaceResourceState::None,
                                workspace_disposal: None,
                                workspace_unresolved: None,
                                // Reconciliation should supply a later terminal. Never
                                // expose an orphan as Active while replay is in progress.
                                lifecycle: SubagentLifecycle::Interrupted,
                                cancel_reason: None,
                                steer_tickets: Vec::new(),
                                deadline_task: None,
                                control: None,
                                detail: None,
                                observation: SubagentObservation::default(),
                                profile: Some(SubagentExecutionProfile::from_frozen(
                                    &spec.resolved.model,
                                )),
                                terminal_workflow_value: None,
                                pending_terminal: None,
                                publication_abandoned: false,
                                notification: NotificationState::Delivered,
                                started_at: envelope.timestamp,
                            });
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
                            let agent = super::SubagentName::parse(&agent).map_err(|error| {
                                ConversationStoreError::InvalidReference(error.to_string())
                            })?;
                            let definition_digest = serde_json::from_value(
                                serde_json::Value::String(definition_digest),
                            )
                            .map_err(|error| {
                                ConversationStoreError::InvalidReference(error.to_string())
                            })?;
                            let index = state.records.len();
                            state.index.insert(subagent_id.clone(), index);
                            state.records.push(SubagentRecord {
                                ownership: crate::events::types::SubagentOwnershipKind::Workflow,
                                terminal: None,
                                physical_settlement_proven: false,
                                input_accepted: false,
                                parent_agent_id,
                                subagent_id,
                                child_agent_id,
                                child_conversation_id,
                                origin,
                                agent,
                                definition_digest,
                                profile_digest:
                                    super::SubagentExecutionProfileDigest::from_committed_fact(
                                        profile_digest,
                                    ),
                                workspace,
                                handoff: None,
                                workspace_resource_state: SubagentWorkspaceResourceState::None,
                                workspace_disposal: None,
                                workspace_unresolved: None,
                                lifecycle: SubagentLifecycle::Interrupted,
                                cancel_reason: None,
                                steer_tickets: Vec::new(),
                                deadline_task: None,
                                control: None,
                                detail: None,
                                observation: SubagentObservation::default(),
                                profile: None,
                                terminal_workflow_value: None,
                                pending_terminal: None,
                                publication_abandoned: false,
                                notification: NotificationState::None,
                                started_at: envelope.timestamp,
                            });
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
                                SubagentTerminalState::Interrupted => {
                                    SubagentLifecycle::Interrupted
                                }
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
                            if let Some(agent) =
                                state.agents.get(&state.records[index].child_agent_id)
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
        }
        for (activation_id, (agent_id, origin)) in pending_admissions {
            if let Some(agent) = state.agents.get_mut(&agent_id) {
                agent.workspace.await_recovered_physical_proof();
                let (completion, _) = tokio::sync::watch::channel(AdmissionSettlement::Failed);
                agent.resuming = Some(ResumeReservation {
                    activation_id: activation_id.clone(),
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
