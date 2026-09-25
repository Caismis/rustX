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

/// Immutable admitted semantic authority. Credential caches stay process-private
/// under the existing frozen-provider serialization contract.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrozenAgentAuthority {
    pub resolved: ResolvedSubagentSpec,
    pub execution_policy: super::super::InheritedExecutionPolicy,
    pub approval_mode: crate::runtime::types::ApprovalMode,
}

impl From<&SubagentStartSpec> for FrozenAgentAuthority {
    fn from(spec: &SubagentStartSpec) -> Self {
        Self {
            resolved: spec.resolved.clone(),
            execution_policy: spec.execution_policy,
            approval_mode: spec.approval_mode,
        }
    }
}

pub(super) struct AgentRecord {
    pub authority: SubagentStartSpec,
    pub workspace: crate::runtime::workspace::AgentWorkspace,
    pub conversation_id: ConversationId,
    pub latest_activation: SubagentId,
    pub resuming: Option<ResumeReservation>,
}

impl AgentRecord {
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

pub(super) struct ResumeReservation {
    pub activation_id: SubagentId,
    pub cancellation: CancellationSignal,
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
    Active,
    Stopping,
    Inactive,
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
    #[error("activation physical settlement or canonical publication failed")]
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
        let lifecycle = if agent.resuming.is_some() {
            AgentState::Stopping
        } else {
            match activation.lifecycle {
                SubagentLifecycle::Running => AgentState::Active,
                lifecycle if lifecycle.is_terminal() => AgentState::Inactive,
                _ => AgentState::Stopping,
            }
        };
        Some(AgentSnapshot {
            agent_id: id.clone(),
            conversation_id: agent.conversation_id.clone(),
            parent_agent_id: activation.parent_agent_id.clone(),
            agent: activation.agent.as_str().to_owned(),
            state: lifecycle,
            current_activation: activation
                .lifecycle
                .is_active()
                .then(|| agent.latest_activation.clone()),
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

    /// Stable identity order, bounded regardless of caller input.
    pub fn list_agents(&self, limit: usize) -> Vec<AgentSnapshot> {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state
            .agents
            .keys()
            .take(limit.min(64))
            .filter_map(|id| Self::agent_snapshot_locked(&state, id))
            .collect()
    }

    /// Capture once under the lifecycle mutex. Later activations cannot
    /// change the wait target. Inactive returns immediately with no target.
    fn capture_agent_activation(
        &self,
        id: &AgentId,
    ) -> Result<Option<SubagentId>, AgentControlError> {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let snapshot = Self::agent_snapshot_locked(&state, id)
            .ok_or_else(|| AgentControlError::Unknown(id.clone()))?;
        if state.agents[id].resuming.is_some() {
            return Err(AgentControlError::Stopping);
        }
        Ok(snapshot.current_activation)
    }

    /// Wait for the activation captured at this operation's observation boundary.
    ///
    /// # Errors
    /// Returns an error for an unknown Agent, admission in progress, or failed
    /// physical settlement/canonical publication.
    pub async fn wait_agent(&self, id: &AgentId) -> Result<AgentWaitResult, AgentControlError> {
        let target = self.capture_agent_activation(id)?;
        let outcome = match &target {
            Some(target) => self.wait_until_settled(target).await,
            None => None,
        };
        if target.is_some() && !outcome.as_ref().is_some_and(|outcome| outcome.settled) {
            return Err(AgentControlError::Settlement);
        }
        Ok(AgentWaitResult {
            agent_id: id.clone(),
            activation_id: target,
            outcome,
        })
    }

    /// Interrupt the captured activation without destroying its Agent identity.
    ///
    /// # Errors
    /// Returns an error for an unknown Agent, admission in progress, or failed
    /// physical settlement/canonical publication.
    pub async fn interrupt_agent(
        &self,
        id: &AgentId,
    ) -> Result<AgentWaitResult, AgentControlError> {
        let target = self.capture_agent_activation(id)?;
        let outcome = if let Some(target) = &target {
            let _ = self.cancel(target, CancellationReason::UserRequested);
            self.wait_until_settled(target).await
        } else {
            None
        };
        if target.is_some() && !outcome.as_ref().is_some_and(|outcome| outcome.settled) {
            return Err(AgentControlError::Settlement);
        }
        Ok(AgentWaitResult {
            agent_id: id.clone(),
            activation_id: target,
            outcome,
        })
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
                Option<crate::runtime::types::LifecycleAdmission>,
            ),
        }
        Self::validate_guidance_message(message).map_err(AgentControlError::Message)?;
        let decision = {
            let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            let agent = state
                .agents
                .get(id)
                .ok_or_else(|| AgentControlError::Unknown(id.clone()))?;
            if agent.resuming.is_some() {
                return Err(AgentControlError::Stopping);
            }
            let activation_id = agent.latest_activation.clone();
            let activation = &state.records[state.index[&activation_id]];
            let decision = match activation.lifecycle {
                SubagentLifecycle::Running => {
                    let (sequence, answer, ticket) = self
                        .admit_guidance_locked(&mut state, &activation_id, message)
                        .map_err(AgentControlError::Message)?;
                    Decision::Deliver(activation_id, sequence, answer, ticket)
                }
                lifecycle if lifecycle.is_terminal() => {
                    let admission =
                        self.config.mailbox.begin_running_admission().map_err(|_| {
                            AgentControlError::Start(SubagentStartError::ConversationInactive)
                        })?;
                    let activation_id = SubagentId::for_conversation(
                        &self.config.conversation_id,
                        state.next_ordinal,
                    );
                    state.next_ordinal += 1;
                    let cancellation = CancellationSignal::new();
                    let agent = state.agents.get_mut(id).expect("looked up under same lock");
                    let mut spec = agent.authority.clone();
                    message.clone_into(&mut spec.task);
                    spec.context = None;
                    agent.resuming = Some(ResumeReservation {
                        activation_id: activation_id.clone(),
                        cancellation: cancellation.clone(),
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
                        admission,
                    )
                }
                _ => return Err(AgentControlError::Stopping),
            };
            if let Some(observer) = &state.observer {
                observer.observe_agent(
                    &Self::agent_snapshot_locked(&state, id).expect("Agent owns activation"),
                );
            }
            self.state_version.send_modify(|version| *version += 1);
            decision
        };
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
            Decision::Resume(spec, identity, cancellation, admission) => {
                // The owner retains staging/rollback even if the caller drops
                // its response future after the reservation committed.
                let registry = self.clone();
                tokio::spawn(async move {
                    let _admission = admission;
                    let result = async {
                        let prepared = registry
                            .prepare_inner(&spec, &cancellation, &mut None, Some(&identity))
                            .await?;
                        registry.commit(prepared, &cancellation).await
                    }
                    .await;
                    {
                        let mut state = registry
                            .state
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner);
                        let agent = state
                            .agents
                            .get_mut(&identity.agent_id)
                            .expect("durable Agent survives activation");
                        agent.finish_resume(&identity.activation_id);
                        if let Some(observer) = &state.observer {
                            observer.observe_agent(
                                &Self::agent_snapshot_locked(&state, &identity.agent_id)
                                    .expect("durable Agent"),
                            );
                        }
                        registry.state_version.send_modify(|version| *version += 1);
                    }
                    match result.map_err(AgentControlError::Start)? {
                        SubagentStartOutcome::Accepted(accepted) => Ok(AgentMessageAccepted {
                            agent_id: identity.agent_id,
                            activation_id: accepted.subagent_id,
                            resumed: true,
                        }),
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
                    RuntimeEvent::SubagentOwnershipCommitted {
                        parent_agent_id,
                        subagent_id,
                        child_agent_id,
                        child_conversation_id,
                        tool_call_id,
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
                        unsettled.insert(subagent_id.clone());
                        if let Some(authority) = admitted_authority {
                            let spec = SubagentStartSpec {
                                resolved: authority.resolved,
                                execution_policy: authority.execution_policy,
                                approval_mode: authority.approval_mode,
                                task: String::new(),
                                context: None,
                                tool_call_id: tool_call_id.clone(),
                                terminal: SubagentTerminalMode::Normal,
                            };
                            let scope = crate::runtime::workspace::AgentWorkspace::recovered(
                                self.config.workspace.clone(),
                                subagent_id.clone(),
                                spec.resolved.workspace_policy,
                                workspace.clone(),
                                false,
                            );
                            state.agents.insert(
                                child_agent_id.clone(),
                                AgentRecord {
                                    authority: spec,
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
                                parent_agent_id,
                                subagent_id,
                                child_agent_id,
                                child_conversation_id,
                                tool_call_id,
                                agent: spec.resolved.agent.clone(),
                                definition_digest: spec.resolved.definition_digest.clone(),
                                profile_digest: spec.resolved.profile_digest(),
                                terminal: SubagentTerminalMode::Normal,
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
                    RuntimeEvent::SubagentTerminalPublished {
                        subagent_id,
                        state: terminal,
                        workspace_resource,
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
                            state.records[index].lifecycle = match terminal {
                                SubagentTerminalState::Succeeded => SubagentLifecycle::Succeeded,
                                SubagentTerminalState::Failed => SubagentLifecycle::Failed,
                                SubagentTerminalState::Cancelled => SubagentLifecycle::Cancelled,
                                SubagentTerminalState::Interrupted => {
                                    SubagentLifecycle::Interrupted
                                }
                            };
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
        for activation in unsettled {
            if let Some(&index) = state.index.get(&activation)
                && let Some(agent) = state.agents.get(&state.records[index].child_agent_id)
            {
                // An orphan without reconciliation evidence is not proof of
                // physical containment. Never silently grant a fresh lease.
                agent.workspace.poison();
            }
        }
        Ok(())
    }
}
