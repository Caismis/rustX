//! The registry remains the settlement owner after a process restart. Every
//! unresolved entry names one exact activation and child physical namespace.
//! Fresh native proof can discharge that obligation without reopening its
//! interrupted logical attempt or changing its terminal publication.

use super::{PoisonError, SubagentRegistry, publish_committed_snapshot};

struct RecoveryObligation {
    activation: super::SubagentId,
    index: usize,
    agent_id: super::AgentId,
    conversation: super::ConversationId,
    origin: Option<super::AgentActivationOrigin>,
}

/// One captured pass owns each obligation until its proof append and snapshot
/// cut finish. Other passes skip it; neither controls nor readers wait for I/O.
struct RecoveryPass {
    registry: SubagentRegistry,
    activations: Vec<super::SubagentId>,
}

impl Drop for RecoveryPass {
    fn drop(&mut self) {
        let mut state = self
            .registry
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        for activation in &self.activations {
            state.recovery_inflight.remove(activation);
        }
    }
}

/// Releasing the reconciler's bounded ownership never asserts physical proof.
/// On panic or cancellation, shutdown can classify the retained obligations.
struct ReconciliationCompletion(tokio::sync::watch::Sender<bool>);

impl Drop for ReconciliationCompletion {
    fn drop(&mut self) {
        self.0.send_replace(true);
    }
}

impl SubagentRegistry {
    /// One bounded runtime owner follows old child drain completion without a
    /// client retry. The deadline limits supervision, never constitutes proof.
    /// Unresolved entries remain explicit and are reconsidered on later opens.
    pub(super) fn start_recovery_reconciliation(&self) {
        if self
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .recovery_pending
            .is_empty()
        {
            return;
        }
        let Ok(executor) = tokio::runtime::Handle::try_current() else {
            return;
        };
        self.recovery_reconciliation.send_replace(false);
        let registry = self.clone();
        let completion = ReconciliationCompletion(self.recovery_reconciliation.clone());
        executor.spawn(async move {
            let _completion = completion;
            let mut ticks = tokio::time::interval(std::time::Duration::from_millis(100));
            for _ in 0..150 {
                ticks.tick().await;
                registry.reconcile_recovered_settlements();
                if registry
                    .state
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .recovery_pending
                    .is_empty()
                {
                    break;
                }
            }
        });
    }

    pub(crate) async fn wait_recovery_reconciliation(&self) {
        let mut completion = self.recovery_reconciliation.subscribe();
        while !*completion.borrow_and_update() {
            if completion.changed().await.is_err() {
                break;
            }
        }
        self.reconcile_recovered_settlements();
    }

    fn capture_recovery_obligations(&self) -> Vec<RecoveryObligation> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let pending: Vec<_> = state
            .recovery_pending
            .iter()
            .filter(|id| !state.recovery_inflight.contains(*id))
            .cloned()
            .collect();
        let mut obligations = Vec::new();
        for activation in pending {
            let (index, agent_id, conversation, origin) =
                if let Some(&index) = state.index.get(&activation) {
                    let record = &state.records[index];
                    (
                        index,
                        record.child_agent_id.clone(),
                        record.child_conversation_id.clone(),
                        None,
                    )
                } else if let Some((agent_id, agent)) = state.agents.iter().find(|(_, agent)| {
                    agent
                        .resuming
                        .as_ref()
                        .is_some_and(|r| r.activation_id == activation)
                }) {
                    (
                        state.index[&agent.latest_activation],
                        agent_id.clone(),
                        agent.conversation_id.clone(),
                        Some(agent.resuming.as_ref().unwrap().origin.clone()),
                    )
                } else {
                    continue;
                };
            state.recovery_inflight.insert(activation.clone());
            obligations.push(RecoveryObligation {
                activation,
                index,
                agent_id,
                conversation,
                origin,
            });
        }
        obligations
    }

    fn recovered_settlement_event(
        &self,
        agent_id: &super::AgentId,
        activation: &super::SubagentId,
        origin: Option<&super::AgentActivationOrigin>,
    ) -> crate::events::types::RuntimeEventEnvelope {
        match origin {
            None => super::super::physical_settlement_event(
                &self.config.conversation_id,
                activation,
                agent_id,
                self.config.clock.now(),
            ),
            Some(origin) => super::super::admission_event(
                &self.config.conversation_id,
                agent_id,
                activation,
                origin,
                crate::events::types::AgentActivationAdmissionPhase::RolledBack {
                    physical_settlement_proven: true,
                },
                self.config.clock.now(),
            ),
        }
    }

    /// A bounded reconciliation pass at startup, Goal idle and runtime drain.
    /// Missing evidence leaves the concrete obligation available for the next
    /// pass/reopen. No inspection of a PID or clean Git tree substitutes for it.
    pub(crate) fn reconcile_recovered_settlements(&self) {
        let obligations = self.capture_recovery_obligations();
        let _pass = RecoveryPass {
            registry: self.clone(),
            activations: obligations
                .iter()
                .map(|obligation| obligation.activation.clone())
                .collect(),
        };
        for RecoveryObligation {
            activation,
            index,
            agent_id,
            conversation,
            origin,
        } in obligations
        {
            #[cfg(test)]
            {
                let hook = self
                    .state
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .recovery_probe_hook
                    .take();
                if let Some(hook) = hook {
                    hook();
                }
            }
            let Ok(Some(_proof)) = super::super::physical_recovery::prove(
                &self.config.spawn.product_root,
                &self.config.spawn.session_id,
                &conversation,
                &activation,
            ) else {
                continue;
            };
            let event = self.recovered_settlement_event(&agent_id, &activation, origin.as_ref());
            let Ok(committed) = self
                .config
                .mailbox
                .commit_subagent_physical_settlement(event)
            else {
                continue;
            };
            // The claim pins an immutable activation, never an Agent's latest
            // generation. Revalidate it after the filesystem/SQLite phase.
            let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            let current = state.recovery_pending.contains(&activation)
                && state.recovery_inflight.contains(&activation)
                && match &origin {
                    Some(_) => state.agents.get(&agent_id).is_some_and(|agent| {
                        agent.conversation_id == conversation
                            && agent
                                .resuming
                                .as_ref()
                                .is_some_and(|r| r.activation_id == activation)
                    }),
                    None => {
                        state.index.get(&activation) == Some(&index)
                            && state.records[index].child_agent_id == agent_id
                            && state.records[index].child_conversation_id == conversation
                    }
                };
            if !current {
                continue;
            }
            // The durable receipt releases precisely this complete owner cut.
            if origin.is_some() {
                let agent = state.agents.get_mut(&agent_id).unwrap();
                if let Some(reservation) = agent.resuming.take() {
                    reservation
                        .completion
                        .send_replace(super::agents::AdmissionSettlement::RolledBack);
                }
            } else {
                state.records[index].physical_settlement_proven = true;
            }
            state.recovery_pending.remove(&activation);
            if let Some(agent) = state.agents.get(&agent_id)
                && !state.recovery_pending.iter().any(|id| {
                    state
                        .index
                        .get(id)
                        .is_some_and(|index| state.records[*index].child_agent_id == agent_id)
                        || agent
                            .resuming
                            .as_ref()
                            .is_some_and(|r| r.activation_id == *id)
                })
            {
                agent.workspace.prove_recovered_physical_settlement();
            }
            publish_committed_snapshot(&mut state, &self.state_version, index, committed.sequence);
            // A physical proof creates no new inbound message. Explicitly wake
            // the existing idle coordinator so an active Goal can re-evaluate.
            self.config.mailbox.wake().notify_one();
            // Retain the immutable authority as exact recovery evidence until
            // Session deletion. It grants no execution or workspace authority.
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ReconciliationCompletion;

    #[tokio::test]
    async fn reconciliation_owner_cancellation_releases_completion_without_physical_proof() {
        let (complete, mut completed) = tokio::sync::watch::channel(false);
        let (entered, entry) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            let _completion = ReconciliationCompletion(complete);
            entered.send(()).unwrap();
            std::future::pending::<()>().await;
        });
        entry.await.unwrap();
        assert!(
            !*completed.borrow(),
            "the parked owner still holds its completion"
        );
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        completed.changed().await.unwrap();
        assert!(
            *completed.borrow(),
            "shutdown can now classify the unchanged obligations"
        );
    }
}
