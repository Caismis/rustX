//! The one ownership commit/rollback linearization point of a prepared child,
//! including the capacity bound and ordered capacity waiting. A failed or
//! cancelled commit tears the staged child down completely; a successful one
//! moves the process handle into the driver task.

use super::super::activity::SubagentObservation;
use super::super::ipc::DelegationFrame;
use super::super::ownership_event;
use super::super::process::PhysicalOutcome;
use super::super::process::PhysicalSettlement;
use super::Decision;
use super::DelegateDelivery;
use super::NotificationState;
use super::SubagentLifecycle;
use super::SubagentRecord;
use super::SubagentRegistry;
use super::SubagentTerminalMode;
use super::agents;
use super::interactions::SubagentInteractionSink;
use super::observation::SubagentActivitySink;
use super::observation::publish_committed_snapshot;
use super::observation::publish_snapshot;
use super::preparation::PreparedSubagent;
use super::preparation::SubagentAccepted;
use super::preparation::SubagentStartError;
use super::preparation::SubagentStartOutcome;
use super::settlement::abort_deadline_task;
use crate::events::types::SubagentOwnershipKind;
use crate::runtime::cancellation::CancellationSignal;
use crate::runtime::identity::SubagentId;
use crate::runtime::types::CancellationReason;
use std::sync::Arc;
use std::sync::PoisonError;

/// Removes precisely one native waiting position on every exit. This is only
/// coordination state: no record, durable ownership or active capacity.
struct CapacityWaitTicket<'a> {
    registry: &'a SubagentRegistry,
    ordinal: u64,
}

impl Drop for CapacityWaitTicket<'_> {
    fn drop(&mut self) {
        let mut state = self
            .registry
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if state.capacity_waiters.remove(&self.ordinal).is_some() {
            self.registry
                .state_version
                .send_modify(|version| *version += 1);
        }
    }
}

/// A dropped waiting caller requests cancellation, not abandonment of staged
/// physical ownership. The native task retains the counted admission until
/// commit/rollback completes; accepted work still uses native settlement.
struct CapacityWaitCaller {
    signal: CancellationSignal,
    registry: SubagentRegistry,
    subagent_id: SubagentId,
    armed: bool,
}

impl Drop for CapacityWaitCaller {
    fn drop(&mut self) {
        if self.armed {
            self.signal.cancel();
            // Serializes with the real commit, covering abandonment after
            // commit but before the caller consumes the acceptance value.
            let _ = self
                .registry
                .cancel(&self.subagent_id, CancellationReason::ParentCancelled);
        }
    }
}

impl SubagentRegistry {
    /// **Commit.** The one commit/rollback linearization point.
    ///
    /// A rolled-back or failed commit tears the staged child down
    /// completely (killed, reaped, runtime root removed) before returning;
    /// a successful commit publishes the durable ownership event, creates
    /// the record, releases the start gate, and returns the tool result.
    ///
    /// # Errors
    ///
    /// Returns [`SubagentStartError::ConversationInactive`] when the
    /// conversation is shutting down, [`SubagentStartError::CapacityExceeded`] when
    /// the active bound is full or ordered waiters precede this admission, or
    /// [`SubagentStartError::Durability`] when the ownership commit fails.
    ///
    /// # Panics
    ///
    /// Panics only if the registry loses its accepted ownership record between
    /// the durable commit and control-handle publication, which would violate
    /// the registry's own ownership invariant.
    #[allow(clippy::too_many_lines)] // One commit path, asserted end to end.
    pub async fn commit(
        &self,
        prepared: PreparedSubagent,
        attempt_cancellation: &CancellationSignal,
    ) -> Result<SubagentStartOutcome, SubagentStartError> {
        self.commit_with_capacity_policy(
            prepared,
            attempt_cancellation,
            false,
            self.config.mailbox.begin_running_admission(),
        )
        .await
    }

    /// Commits an actual prepared child in registered native ordinal order.
    /// The first blocked decision inserts its position under `RegistryState`;
    /// only the smallest registered ordinal may attempt ownership commit.
    /// Watch notifications only trigger eligibility rechecks, never reordering.
    /// The staged child holds no active capacity. Cancellation conclusively
    /// rolls it back; the caller must await this operation to settlement.
    /// Waiting is bounded by the invoking foreground execution's cancellation
    /// authority (including its finite execution deadline).
    pub(crate) async fn commit_waiting(
        &self,
        prepared: PreparedSubagent,
        cancellation: &CancellationSignal,
    ) -> Result<SubagentStartOutcome, SubagentStartError> {
        let mut caller = CapacityWaitCaller {
            signal: cancellation.child(),
            registry: self.clone_for_task(),
            subagent_id: prepared.subagent_id.clone(),
            armed: true,
        };
        let signal = caller.signal.clone();
        let registry = self.clone_for_task();
        // Count synchronously before task handoff: drain must not observe a
        // gap while this privately staged child still requires rollback.
        let admission = self.config.mailbox.begin_running_admission();
        let operation_registry = registry.clone_for_task();
        let mut operation = Box::pin(async move {
            operation_registry
                .commit_with_capacity_policy(prepared, &signal, true, admission)
                .await
        });
        // Preserve the existing synchronous initial ownership/registration
        // frontier. Task handoff must not delay that decision behind later
        // native submissions. Only an operation that actually suspends needs
        // an independent owner for abandonment-safe rollback. Subsequent
        // eligibility is still exclusively the registry's ordinal rule.
        if let std::task::Poll::Ready(result) = std::future::poll_fn(|cx| {
            std::task::Poll::Ready(std::future::Future::poll(operation.as_mut(), cx))
        })
        .await
        {
            caller.armed = false;
            return result;
        }
        let (sender, receiver) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let result = operation.await;
            if let Err(result) = sender.send(result) {
                match result {
                    Ok(SubagentStartOutcome::Accepted(accepted)) => {
                        let _ = registry
                            .cancel(&accepted.subagent_id, CancellationReason::ParentCancelled);
                        let _ = registry.wait_until_settled(&accepted.subagent_id).await;
                    }
                    Err(error) => {
                        tracing::error!(%error, "abandoned native capacity wait failed settlement");
                    }
                    Ok(SubagentStartOutcome::RolledBack) => {}
                }
            }
            #[cfg(test)]
            if let Some(finished) = registry
                .state
                .lock()
                .expect("registry state")
                .capacity_wait_finished
                .take()
            {
                let _ = finished.send(());
            }
        });
        let result = receiver
            .await
            .expect("native capacity commit task must settle");
        caller.armed = false;
        result
    }

    #[cfg(test)]
    pub(crate) fn watch_next_capacity_wait(&self) -> tokio::sync::oneshot::Receiver<()> {
        let (sender, receiver) = tokio::sync::oneshot::channel();
        self.state
            .lock()
            .expect("registry state")
            .capacity_wait_entered = Some(sender);
        receiver
    }

    #[allow(clippy::too_many_lines)]
    async fn commit_with_capacity_policy(
        &self,
        prepared: PreparedSubagent,
        attempt_cancellation: &CancellationSignal,
        wait_for_capacity: bool,
        admission: Result<
            Option<crate::runtime::types::LifecycleAdmission>,
            crate::runtime::inbound::MailboxError,
        >,
    ) -> Result<SubagentStartOutcome, SubagentStartError> {
        // Retain the counted lifecycle admission through the entire
        // prepared-to-driver handoff, including conclusive rollback. This
        // prevents runtime drain from declaring quiescence between the
        // durable ownership decision and publication of the driver control
        // path; the registry's own cancellation state still handles a drain
        // that wins after the record is visible.
        let Ok(_admission) = admission else {
            return match prepared.staged.rollback().await {
                Ok(()) => Err(SubagentStartError::ConversationInactive),
                Err(error) => Err(SubagentStartError::Rollback {
                    detail: error.to_string(),
                }),
            };
        };
        let PreparedSubagent {
            authority,
            workspace,
            subagent_id,
            child_agent_id,
            child_conversation_id,
            origin,
            agent,
            definition_digest,
            profile_digest,
            terminal,
            task,
            context,
            attachments,
            execution_deadline,
            profile,
            staged,
        } = prepared;
        let ordinal = subagent_id
            .conversation_ordinal(&self.config.conversation_id)
            .expect("prepared child has a native conversation ordinal");
        let ticket = CapacityWaitTicket {
            registry: self,
            ordinal,
        };
        let mut capacity_changes = self.state_version.subscribe();
        let decision = loop {
            capacity_changes.borrow_and_update();
            // Product ownership admission precedes every registry, durability
            // and lifecycle commit mutex. A read-only snapshot delays this
            // transition without making the staged child fail. The existing
            // arbitration below rechecks cancellation, capacity, drain and
            // durability after admission, before any ownership publication.
            let ownership_admission = match self
                .config
                .spawn
                .product_root
                .runtime_ownership_admission()
                .await
            {
                Ok(admission) => admission,
                Err(error) => {
                    break Decision::Failed(SubagentStartError::Durability {
                        detail: error.to_string(),
                    });
                }
            };
            let mailbox = self.config.mailbox.clone();
            let config = &self.config;
            // Reserve capacity and the exact generation before releasing the
            // linearization mutex. Controls can cancel this owner while SQLite
            // is parked; another admission cannot steal its capacity.
            let selection = {
                let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
                let active = state
                    .records
                    .iter()
                    .filter(|record| record.lifecycle.is_active())
                    .count()
                    + state.ownership_committing.len();
                if attempt_cancellation.is_cancelled() {
                    Err(Decision::RolledBack)
                } else if state.goal_idle_committing {
                    Err(Decision::WaitingForGoalIdle)
                } else {
                    if wait_for_capacity
                        && state.max_active > 0
                        && (active >= state.max_active || !state.capacity_waiters.is_empty())
                    {
                        state
                            .capacity_waiters
                            .entry(ordinal)
                            .or_insert_with(|| attempt_cancellation.clone());
                    }
                    let eligible = state
                        .capacity_waiters
                        .first_key_value()
                        .is_none_or(|(first, _)| wait_for_capacity && *first == ordinal);
                    if active >= state.max_active || !eligible {
                        Err(Decision::Failed(SubagentStartError::CapacityExceeded {
                            max: state.max_active,
                        }))
                    } else {
                        let first = matches!(terminal, SubagentTerminalMode::Normal)
                            && !state.agents.contains_key(&child_agent_id);
                        assert!(
                            state
                                .ownership_committing
                                .insert(subagent_id.clone(), (attempt_cancellation.clone(), None))
                                .is_none()
                        );
                        Ok((first, state.durability_gate.clone()))
                    }
                }
            };
            let decision = match selection {
                Err(decision) => decision,
                Ok((first_admission, durability_gate)) => {
                    let ownership_permission = durability_gate
                        .as_ref()
                        .map(|gate| gate.enter_ownership_commit());
                    let decision = if let Some(Err(refused)) = &ownership_permission {
                        Decision::Failed(SubagentStartError::DurabilityFailed {
                            detail: refused.diagnostic.clone(),
                        })
                    } else {
                        mailbox.with_running_commit(|| {
                            if mailbox.is_bound_inactive() {
                                return Decision::Failed(SubagentStartError::ConversationInactive);
                            }
                            #[cfg(test)]
                            let hook = self.state.lock().unwrap_or_else(PoisonError::into_inner).commit_hook.clone();
                            #[cfg(test)]
                            if let Some(hook) = hook { hook.wait(); }
                            if attempt_cancellation.is_cancelled() {
                                return Decision::RolledBack;
                            }
                            let started_at = config.clock.now();
                            let mut ownership = ownership_event(
                                &config.agent_id, &config.conversation_id, &subagent_id,
                                &child_agent_id, &child_conversation_id, &origin, &agent,
                                &definition_digest, &profile_digest,
                                match &terminal {
                                    SubagentTerminalMode::Normal => SubagentOwnershipKind::Normal,
                                    SubagentTerminalMode::WorkflowOutput { .. } => SubagentOwnershipKind::Workflow,
                                }, &workspace, started_at,
                            );
                            if first_admission
                                && let crate::events::types::RuntimeEvent::SubagentOwnershipCommitted { admitted_authority, .. } = &mut ownership.event
                            { *admitted_authority = Some(child_agent_id.clone()); }
                            match mailbox.commit_subagent_ownership(&ownership_admission, ownership, first_admission.then_some(&authority)) {
                                Ok(committed) => Decision::Accepted {
                                    started_at,
                                    deadline_at_millis: execution_deadline.map(|deadline| config.monotonic_clock.now_millis().saturating_add(deadline.as_millis())),
                                    committed: Box::new(committed),
                                },
                                Err(error) => Decision::Failed(SubagentStartError::Durability { detail: error.to_string() }),
                            }
                        }).unwrap_or(Decision::Failed(SubagentStartError::ConversationInactive))
                    };
                    let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
                    let (_, committed_cancellation) = state
                        .ownership_committing
                        .remove(&subagent_id)
                        .expect("exact ownership committer retains its capacity reservation");
                    if let Decision::Accepted {
                        started_at,
                        committed,
                        ..
                    } = &decision
                    {
                        if matches!(terminal, SubagentTerminalMode::Normal) {
                            if !state.agents.contains_key(&child_agent_id) {
                                let ordinal = state.agent_observation_order.len();
                                state
                                    .agent_observation_index
                                    .insert(child_agent_id.clone(), ordinal);
                                state.agent_observation_order.push(child_agent_id.clone());
                                state
                                    .observation_layouts
                                    .insert(child_agent_id.clone(), Arc::default());
                            }
                            state
                                .agents
                                .entry(child_agent_id.clone())
                                .and_modify(|agent| {
                                    agent.latest_activation = subagent_id.clone();
                                    agent.finish_resume(&subagent_id);
                                })
                                .or_insert_with(|| agents::AgentRecord {
                                    created_sequence: committed.sequence,
                                    authority: Arc::new(authority.clone()),
                                    workspace: workspace.clone(),
                                    conversation_id: child_conversation_id.clone(),
                                    latest_activation: subagent_id.clone(),
                                    resuming: None,
                                });
                        }
                        let cancelled = committed_cancellation.or_else(|| {
                            attempt_cancellation
                                .is_cancelled()
                                .then_some(CancellationReason::ParentCancelled)
                        });
                        let record = SubagentRecord {
                            execution: None,
                            physical_settlement_proven: false,
                            delegate_delivery: DelegateDelivery::NotSent,
                            parent_agent_id: config.agent_id.clone(),
                            subagent_id: subagent_id.clone(),
                            child_agent_id: child_agent_id.clone(),
                            child_conversation_id: child_conversation_id.clone(),
                            origin: origin.clone(),
                            agent: agent.clone(),
                            definition_digest: definition_digest.clone(),
                            profile_digest: profile_digest.clone(),
                            ownership: match terminal {
                                SubagentTerminalMode::Normal => SubagentOwnershipKind::Normal,
                                SubagentTerminalMode::WorkflowOutput { .. } => {
                                    SubagentOwnershipKind::Workflow
                                }
                            },
                            terminal: Some(terminal.clone()),
                            workspace: workspace.clone(),
                            lifecycle: if cancelled.is_some() {
                                SubagentLifecycle::Cancelling
                            } else {
                                SubagentLifecycle::Starting
                            },
                            cancel_reason: cancelled,
                            steer_tickets: Vec::new(),
                            deadline_task: None,
                            control: None,
                            detail: None,
                            observation: SubagentObservation::default(),
                            profile: Some(Arc::new(profile.clone())),
                            terminal_workflow_value: None,
                            pending_terminal: None,
                            publication_abandoned: false,
                            notification: NotificationState::None,
                            started_at: *started_at,
                        };
                        let index = state.records.len();
                        state.index.insert(subagent_id.clone(), index);
                        state.records.push(record);
                        publish_committed_snapshot(
                            &mut state,
                            &self.state_version,
                            index,
                            committed,
                        );
                    }
                    self.state_version.send_modify(|version| *version += 1);
                    decision
                }
            };
            // The durable event and Starting record are now one published fact.
            // Never retain this guard during capacity waits, rollback or driver
            // handoff. Archive may capture before or after this whole transition.
            drop(ownership_admission);
            if matches!(decision, Decision::WaitingForGoalIdle)
                || (wait_for_capacity
                    && self
                        .state
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .max_active
                        > 0
                    && matches!(
                        decision,
                        Decision::Failed(SubagentStartError::CapacityExceeded { .. })
                    ))
            {
                #[cfg(test)]
                {
                    let pause = {
                        let mut state = self.state.lock().expect("registry state");
                        if let Some(sender) = state.capacity_wait_entered.take() {
                            let _ = sender.send(());
                        }
                        state.capacity_wait_pause.take()
                    };
                    if let Some(pause) = pause {
                        let _ = pause.await;
                    }
                }
                tokio::select! {
                    biased;
                    () = attempt_cancellation.cancelled() => break Decision::RolledBack,
                    changed = capacity_changes.changed() => {
                        if changed.is_err() { break Decision::Failed(SubagentStartError::ConversationInactive); }
                    }
                }
            } else {
                break decision;
            }
        };
        // Success has published its record before freeing the coordination
        // position. Failure/cancellation frees it before physical rollback.
        // Removal notifies the successor even if no child is active.
        drop(ticket);
        match decision {
            Decision::WaitingForGoalIdle => {
                unreachable!("the idle claim is awaited before an ownership decision")
            }
            Decision::RolledBack => match staged.rollback().await {
                Ok(()) => Ok(SubagentStartOutcome::RolledBack),
                Err(error) => Err(SubagentStartError::Rollback {
                    detail: error.to_string(),
                }),
            },
            Decision::Failed(error) => match staged.rollback().await {
                Ok(()) => Err(error),
                Err(rollback) => Err(SubagentStartError::Rollback {
                    detail: format!("{error}; {rollback}"),
                }),
            },
            Decision::Accepted {
                deadline_at_millis, ..
            } => {
                // The driver routes the child's live activity frames into
                // the registry read model through this narrow sink; it is
                // observation-plane traffic only.
                let activity = SubagentActivitySink {
                    subagent_id: subagent_id.clone(),
                    registry: self.clone_for_task(),
                };
                let interactions = SubagentInteractionSink {
                    subagent_id: subagent_id.clone(),
                    registry: self.clone_for_task(),
                };
                let provider_available = self.interaction_provider_receiver();
                let driver = staged.into_driver(
                    DelegationFrame {
                        attachments,
                        task,
                        context,
                        // The driver overwrites this with the watch's current
                        // value immediately before sending Delegate. Keeping
                        // the field explicit makes the wire contract clear
                        // without creating a second provider authority.
                        interaction_provider_available: false,
                    },
                    Some(activity),
                    Some(interactions),
                    Some(provider_available),
                );
                let execution = driver.execution();
                let (commands, start_gate, task) = driver.split();
                // The task is created only after the durable ownership event
                // and Starting record exist. It calls the same synchronous
                // registry cancellation authority as an explicit cancel;
                // it never creates a terminal result or sends a driver
                // command directly.
                let deadline_task = deadline_at_millis.map(|deadline_at_millis| {
                    let clock = self.config.monotonic_clock.clone();
                    let registry = self.clone_for_task();
                    let deadline_id = subagent_id.clone();
                    // Test-only: claim the test's one-shot completion latch
                    // (if any) for this child, so a fired deadline's full
                    // return is awaitable by the race regression.
                    #[cfg(test)]
                    let completion = {
                        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
                        state.deadline_completion.remove(&deadline_id)
                    };
                    tokio::spawn(async move {
                        clock.wait_until_millis(deadline_at_millis).await;
                        let _ = registry.cancel(
                            &deadline_id,
                            CancellationReason::SubagentExecutionDeadlineExceeded,
                        );
                        // Test-only: fires only after the deadline's
                        // cancellation call has fully returned.
                        #[cfg(test)]
                        if let Some(completion) = completion {
                            let _ = completion.send(());
                        }
                    })
                });
                // This hook is outside the registry lock and after the
                // durable ownership fact, the Starting record, and the
                // driver task all exist. It pauses before the gate-release
                // critical section, so a concurrent cancellation commits
                // while the command handle is still None — the
                // deterministic "cancel lock wins first" edge.
                #[cfg(test)]
                let control_handoff_hook = {
                    self.state
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .control_handoff_hook
                        .clone()
                };
                #[cfg(test)]
                if let Some(hook) = control_handoff_hook {
                    hook.wait();
                }

                // Point of no return: the child is conversation-owned. The
                // OS handle moves into the driver task; the registry keeps
                // only the narrow command handle.
                //
                // One synchronization point: the command-handle install,
                // the lifecycle read, and the start-gate release all happen
                // under the registry mutex, so the mutex is the exact
                // arbitration boundary between start-gate release and
                // explicit cancellation. A cancellation that acquired the
                // mutex first resolved the gate cancelled — the driver
                // sends Cancel before Delegate and never allows child
                // semantic work to begin. A gate release that acquired the
                // mutex first defines an already-started child whose later
                // cancellation is in-flight cancellation.
                let deadline_task_to_abort = {
                    let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
                    // Test-only: extract the pause handle before the record
                    // borrow so the gate-release section is one critical
                    // section.
                    #[cfg(test)]
                    let gate_release_hook = state.gate_release_hook.clone();
                    let index = *state
                        .index
                        .get(&subagent_id)
                        .expect("accepted ownership has a registry record");
                    let record = &mut state.records[index];
                    record.execution = Some(execution);
                    record.control = Some(commands);
                    if matches!(record.lifecycle, SubagentLifecycle::Starting) {
                        record.lifecycle = SubagentLifecycle::Running;
                    }
                    record.deadline_task = deadline_task;
                    // Test-only: the exact remaining edge — the command
                    // handle is installed but the start gate is not yet
                    // released. The pause parks while holding the registry
                    // mutex, so a concurrent `cancel` provably blocks: the
                    // edge is unobservable, never best-effort. Production
                    // has no pause and no equivalent semantic state.
                    #[cfg(test)]
                    if let Some(hook) = gate_release_hook {
                        hook.wait();
                    }
                    let cancel_before_start =
                        matches!(record.lifecycle, SubagentLifecycle::Cancelling).then(|| {
                            record
                                .cancel_reason
                                .expect("cancelling child has a committed reason")
                        });
                    let deadline_task_to_abort = cancel_before_start
                        .is_some()
                        .then(|| record.deadline_task.take())
                        .flatten();
                    // Sending `Some(reason)` resolves the gate cancelled:
                    // the driver sends that exact reason before Delegate
                    // and never allows child semantic work to begin.
                    // Sending `None` opens the normal gate. The release is
                    // synchronous under the same mutex acquisition, so
                    // start-vs-cancel has exactly one arbitration boundary.
                    let _ = start_gate.send(cancel_before_start);
                    publish_snapshot(&mut state, &self.state_version, index);
                    deadline_task_to_abort
                };
                abort_deadline_task(deadline_task_to_abort);
                let registry = self.clone_for_task();
                let settlement_id = subagent_id.clone();
                tokio::spawn(async move {
                    let settlement = task.await.unwrap_or_else(|_| {
                        PhysicalSettlement::of(PhysicalOutcome::ControlFailure {
                            diagnostic: "the child driver task failed".to_owned(),
                        })
                    });
                    registry.settle_from_driver(&settlement_id, settlement);
                });
                Ok(SubagentStartOutcome::Accepted(SubagentAccepted {
                    subagent_id,
                    child_agent_id,
                    child_conversation_id,
                    agent: agent.as_str().to_owned(),
                    definition_digest: definition_digest.as_str().to_owned(),
                }))
            }
        }
    }
}
