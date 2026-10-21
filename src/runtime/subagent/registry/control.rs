//! Parent-authored control of live children: cancellation intent, sealing,
//! and steer/guidance admission. Every decision is taken under the registry
//! mutex shared with ownership commit and terminal settlement.

use super::super::MAX_TASK_BYTES;
use super::RegistryState;
use super::SteerTicket;
use super::SteerTicketPhase;
use super::SubagentLifecycle;
use super::SubagentRecord;
use super::SubagentRegistry;
use super::SubagentTerminalMode;
use super::observation::SubagentSnapshot;
use super::observation::SubagentState;
use super::observation::publish_snapshot;
use super::settlement::abort_deadline_task;
use crate::runtime::identity::SubagentId;
use crate::runtime::types::CancellationReason;
use crate::runtime::workflow::WorkflowId;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::PoisonError;

/// Why one parent-authored steer could not be durably accepted (Issue #193).
///
/// Every variant is deterministic and bounded: a steer is never silently
/// ignored, and `Ok` is never returned unless the child conversation
/// durably accepted the guidance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubagentSteerError {
    /// No such child is owned by this conversation. A cross-conversation id
    /// is indistinguishable from an unknown id by construction.
    Unknown {
        /// The named child.
        subagent_id: SubagentId,
    },
    /// The guidance input is empty or its text exceeds [`MAX_TASK_BYTES`].
    InvalidMessage {
        /// The offending byte length.
        bytes: usize,
    },
    /// Cancellation committed before guidance admission. No new message can
    /// enter that activation after this boundary; already admitted messages
    /// retain their own durable outcome.
    CancellationCommitted {
        /// The committed first-winner cause.
        reason: CancellationReason,
    },
    /// The child is a Workflow-owned `AgentRun`. Its semantic input is
    /// authored by the compiled Workflow program through the
    /// `WorkflowRuntime`, so the generic control plane never authors into
    /// that conversation. This is an ownership fact, not a lifecycle one:
    /// it holds for every state of such a child, and it is decided before
    /// any guidance envelope exists.
    WorkflowOwned {
        /// The Workflow that owns this child's semantic input.
        workflow_id: WorkflowId,
        /// The Workflow node that owns this `AgentRun`.
        node_id: Box<crate::runtime::workflow::WorkflowNodeInstance>,
    },
    /// Terminal authority already won: the child is settling or settled, so
    /// no semantic input can still reach an Agent Loop boundary.
    Settled {
        /// The authoritative lifecycle state at the refusal.
        state: SubagentState,
    },
    /// The child conversation itself refused the guidance — its terminal
    /// seal or its cancellation intent committed first inside the child, or
    /// its durable authority refused the acceptance.
    ChildRefused {
        /// The bounded deterministic refusal reason.
        detail: String,
    },
    /// The child's control plane could not take the envelope — the driver
    /// is gone. The reliable owner queue never rejects due to saturation; the
    /// guidance never reached the child conversation, so nothing was
    /// accepted and no ticket was armed.
    ControlLost,
}

impl core::fmt::Display for SubagentSteerError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Unknown { subagent_id } => {
                write!(f, "unknown subagent execution {subagent_id}")
            }
            Self::InvalidMessage { bytes } => write!(
                f,
                "the guidance input must contain text or attachments and at most {MAX_TASK_BYTES} text bytes ({bytes} bytes given)"
            ),
            Self::CancellationCommitted { reason } => write!(
                f,
                "the subagent cancellation intent is committed ({reason:?}): a cancelled child is never steered"
            ),
            Self::WorkflowOwned {
                workflow_id,
                node_id,
            } => write!(
                f,
                "steering is not supported for workflow-owned subagent executions: this child is the AgentRun of workflow {workflow_id} node {node_id}, whose semantic input is authored by the workflow"
            ),
            Self::Settled { state } => write!(
                f,
                "the subagent is no longer running (state {state:?}): its terminal authority already won"
            ),
            Self::ChildRefused { detail } => {
                write!(f, "the child conversation refused the guidance: {detail}")
            }
            Self::ControlLost => f.write_str(
                "the subagent control plane did not take the guidance: nothing was delivered to the child",
            ),
        }
    }
}

impl std::error::Error for SubagentSteerError {}

/// The **cancellation-safe owned ticket** of one admitted parent-authored
/// steer (Issue #193).
///
/// Message admission arms a ticket under the Agent owner mutex. The message
/// future retains this guard while waiting for the child's durable answer.
/// Dropping that future removes only its own ticket; it cannot withdraw an
/// admitted message, cancel the activation, or erase durable acceptance.
///
/// Cancellation and terminal settlement may clear the same ticket under the
/// owner mutex. A later guard drop then finds nothing to remove. The guard holds
/// only shared registry state, never a process or workspace lifetime lease.
pub(crate) struct GuidanceTicket {
    /// The registry state the ticket was armed in. Only the shared `Arc` is
    /// held, so the guard can never keep the child's driver task, process,
    /// or workspace alive by itself.
    state: Arc<Mutex<RegistryState>>,
    /// The child the ticket belongs to.
    subagent_id: SubagentId,
    /// The ticket's own transport correlation identity.
    guidance_id: u64,
    pub(super) write_started: Arc<std::sync::atomic::AtomicBool>,
}

impl Drop for GuidanceTicket {
    fn drop(&mut self) {
        // Caller abandonment: remove exactly this ticket if it is still
        // outstanding. Cancellation, the terminal authority, or the steer's
        // own commit may already have removed it (all under this same
        // mutex), in which case the drop is a no-op — removal is idempotent
        // because it is keyed by the exact guidance id. The section is short
        // and in-memory; no async work happens in `Drop`.
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(&index) = state.index.get(&self.subagent_id) else {
            return;
        };
        if let Some(position) = state.records[index]
            .steer_tickets
            .iter()
            .position(|ticket| ticket.guidance_id == self.guidance_id)
        {
            state.records[index].steer_tickets.remove(position);
        }
    }
}

impl SubagentRegistry {
    /// **Cancellation.** Commits the cancellation intent under the
    /// registry lock and forwards it into the driver task.
    ///
    /// The call is synchronous and never blocks on process teardown: the
    /// driver owns the Cancel frame, the escalation, and the reap; the
    /// terminal settlement follows through [`Self::settle_from_driver`].
    /// Cancelling an unknown, terminal, or abandoned record is a no-op
    /// returning the current snapshot.
    #[must_use]
    pub fn cancel(
        &self,
        subagent_id: &SubagentId,
        reason: CancellationReason,
    ) -> Option<SubagentSnapshot> {
        self.cancel_at_cut(subagent_id, reason, SubagentRecord::snapshot)
            .unwrap_or_else(|_| self.snapshot(subagent_id))
    }

    pub(super) fn cancel_intent(
        &self,
        subagent_id: &SubagentId,
        reason: CancellationReason,
    ) -> Result<Option<SubagentId>, crate::runtime::inbound::MailboxError> {
        self.cancel_at_cut(subagent_id, reason, |record| record.subagent_id.clone())
    }

    fn cancel_at_cut<T>(
        &self,
        subagent_id: &SubagentId,
        reason: CancellationReason,
        project: impl FnOnce(&SubagentRecord) -> T,
    ) -> Result<Option<T>, crate::runtime::inbound::MailboxError> {
        let _settlement = self.config.mailbox.begin_settlement_admission()?;
        // Test-only: park the first live cancellation contender (the fired
        // deadline task or an explicit caller) immediately before the
        // registry mutex that commits `Running -> Cancelling`. The pause
        // holds no registry lock, so a second cancellation source — or
        // terminal settlement — can acquire the boundary first; the parked
        // contender's release then observes the already-committed
        // transition and cannot overwrite the winner.
        #[cfg(test)]
        let cancellation_boundary_hook = {
            let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            state.cancellation_boundary_hook.clone()
        };
        #[cfg(test)]
        if let Some(hook) = cancellation_boundary_hook {
            hook.wait();
        }
        let (snapshot, deadline_task) = {
            let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            let Some(&index) = state.index.get(subagent_id) else {
                return Ok(None);
            };
            let record = &mut state.records[index];
            if record.lifecycle.is_terminal() || record.publication_abandoned {
                return Ok(Some(project(record)));
            }
            let deadline_task = if record.lifecycle.is_active() {
                record.deadline_task.take()
            } else {
                None
            };
            if matches!(
                record.lifecycle,
                SubagentLifecycle::Starting
                    | SubagentLifecycle::Running
                    | SubagentLifecycle::Stopping
            ) {
                // This mutex transition is the cancellation linearization
                // point. The first source to change Running to Cancelling
                // owns the reason; later sources can only observe it.
                record.lifecycle = SubagentLifecycle::Cancelling;
                record.cancel_reason = Some(reason);
                // Close local guidance tracking at the same cancellation cut.
                // Clearing a ticket cannot erase canonical child acceptance or
                // turn an unacknowledged write into proven non-delivery.
                record.steer_tickets.clear();
                if let Some(control) = &record.control {
                    let _ = control.send(super::super::process::DriverCommand::Cancel { reason });
                }
                publish_snapshot(&mut state, &self.state_version, index);
            }
            (project(&state.records[index]), deadline_task)
        };
        abort_deadline_task(deadline_task);
        Ok(Some(snapshot))
    }

    /// Close message admission under the same mutex as `admit_guidance_locked`.
    /// The driver drains admitted FIFO messages before granting the child seal.
    pub(super) fn begin_seal(&self, subagent_id: &SubagentId) {
        #[cfg(test)]
        let hook;
        {
            let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            let Some(&index) = state.index.get(subagent_id) else {
                return;
            };
            if !matches!(state.records[index].lifecycle, SubagentLifecycle::Running) {
                return;
            }
            state.records[index].lifecycle = SubagentLifecycle::Stopping;
            publish_snapshot(&mut state, &self.state_version, index);
            #[cfg(test)]
            {
                hook = state.seal_boundary_hook.take();
            }
        }
        // The native Stop admission is committed; the driver has not granted
        // the physical child its seal yet. The hook never holds registry locks.
        #[cfg(test)]
        if let Some(hook) = hook {
            hook.wait();
        }
    }

    /// Shared task/guidance content admission, before ownership or preparation.
    pub(super) fn valid_input(message: &str, attachments: usize) -> bool {
        (!message.trim().is_empty() || attachments > 0) && message.len() <= MAX_TASK_BYTES
    }

    /// Advances one ticket to
    /// [`SteerTicketPhase::ChildAcceptedPendingParentCommit`] once the
    /// child's durable `Accepted` answer has reached the steer caller.
    ///
    /// This is the point where the registry's ticket state learns that the
    /// steer's semantic effect is committed while its parent acknowledgement
    /// commit is still owed. It runs under the arbitration mutex, so it
    /// linearizes against cancellation and terminal authority. If the
    /// ticket is already gone — cancellation or the terminal authority
    /// disposed of it while this caller was not scheduled — there is
    /// nothing to advance: the accepted classification is preserved by the
    /// caller-held `child_outcome` through the commit, which cleanup cannot
    /// erase.
    pub(super) fn record_child_decision(
        &self,
        subagent_id: &SubagentId,
        guidance_id: u64,
        child_outcome: &Result<
            super::super::ipc::ChildGuidanceOutcome,
            tokio::sync::oneshot::error::RecvError,
        >,
    ) {
        if !matches!(
            child_outcome,
            Ok(super::super::ipc::ChildGuidanceOutcome::Accepted)
        ) {
            return;
        }
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(&index) = state.index.get(subagent_id) else {
            return;
        };
        if let Some(ticket) = state.records[index]
            .steer_tickets
            .iter_mut()
            .find(|ticket| ticket.guidance_id == guidance_id)
        {
            ticket.phase = SteerTicketPhase::ChildAcceptedPendingParentCommit;
        }
    }

    /// **Effect-frontier phase of steering:** arbitrate ownership and
    /// lifecycle, hand the envelope to the driver, arm the steer ticket, and
    /// create its ownership guard — all inside the one arbitration mutex, so
    /// no cancellation can interleave between the hand-off and the ticket.
    ///
    /// This is the steer **effect frontier**: after it returns `Ok`, the
    /// envelope is provably on the driver's lane and the ticket is armed, so
    /// the child conversation may durably accept the guidance at any time; a
    /// refusal returns before any envelope exists and arms nothing.
    ///
    /// The returned [`GuidanceTicket`] is the one owner of the ticket's
    /// terminal disposition when the steer future is dropped before commit;
    /// dropping it removes exactly its own ticket under this same mutex.
    ///
    /// The message must already be validated by
    /// the shared input policy. Production admission is composed
    /// inside `send_message` under the same Agent owner mutex.
    #[cfg(test)]
    pub(super) fn admit_guidance(
        &self,
        subagent_id: &SubagentId,
        message: &str,
        attachments: &[crate::message::content::UploadedFileRef],
    ) -> Result<
        (
            u64,
            tokio::sync::oneshot::Receiver<super::super::ipc::ChildGuidanceOutcome>,
            GuidanceTicket,
        ),
        SubagentSteerError,
    > {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        self.admit_guidance_locked(&mut state, subagent_id, message, attachments)
    }

    pub(super) fn admit_guidance_locked(
        &self,
        state: &mut RegistryState,
        subagent_id: &SubagentId,
        message: &str,
        attachments: &[crate::message::content::UploadedFileRef],
    ) -> Result<
        (
            u64,
            tokio::sync::oneshot::Receiver<super::super::ipc::ChildGuidanceOutcome>,
            GuidanceTicket,
        ),
        SubagentSteerError,
    > {
        {
            let Some(&index) = state.index.get(subagent_id) else {
                return Err(SubagentSteerError::Unknown {
                    subagent_id: subagent_id.clone(),
                });
            };
            let record = &mut state.records[index];
            // Ownership outranks lifecycle. A Workflow-owned `AgentRun`'s
            // semantic input — profile, task, bound inputs, frozen output
            // schema — belongs to the compiled Workflow program, so the
            // generic control plane must not author into that conversation
            // at all. The refusal is structural and therefore checked before
            // anything else about this child's state, and provably before
            // any Guidance frame exists.
            if let Some(SubagentTerminalMode::WorkflowOutput {
                workflow_id,
                node_id,
                ..
            }) = &record.terminal
            {
                return Err(SubagentSteerError::WorkflowOwned {
                    workflow_id: workflow_id.clone(),
                    node_id: node_id.clone(),
                });
            }
            // Reading the lifecycle inside this mutex — rather than as an
            // unrelated `is_running()` probe — is what gives the refusal a
            // total order against both cancellation and terminal authority.
            match record.lifecycle {
                SubagentLifecycle::Running => {}
                SubagentLifecycle::Cancelling => {
                    return Err(SubagentSteerError::CancellationCommitted {
                        reason: record
                            .cancel_reason
                            .unwrap_or(CancellationReason::UserRequested),
                    });
                }
                SubagentLifecycle::Starting
                | SubagentLifecycle::Stopping
                | SubagentLifecycle::PublishingTerminal
                | SubagentLifecycle::Succeeded
                | SubagentLifecycle::Failed
                | SubagentLifecycle::Cancelled
                | SubagentLifecycle::Interrupted => {
                    return Err(SubagentSteerError::Settled {
                        state: record.snapshot().state,
                    });
                }
            }
            let Some(control) = record.control.clone() else {
                return Err(SubagentSteerError::ControlLost);
            };
            let guidance_id = state.next_guidance_id;
            state.next_guidance_id = guidance_id.saturating_add(1);
            let (outcome, receiver) = tokio::sync::oneshot::channel();
            let write_started = Arc::new(std::sync::atomic::AtomicBool::new(false));
            if control
                .send(super::super::process::DriverCommand::Route(
                    super::super::process::ChildBoundRoute::Guidance {
                        guidance_id,
                        message: message.to_owned(),
                        attachments: attachments.to_vec(),
                        write_started: Arc::clone(&write_started),
                        outcome,
                    },
                ))
                .is_err()
            {
                return Err(SubagentSteerError::ControlLost);
            }
            // The ticket is armed only once the envelope is provably on the
            // driver's lane, and inside the same critical section. Its
            // ownership guard is created here, under the same mutex, so a
            // ticket can never exist without an owner. A new ticket always
            // starts in [`SteerTicketPhase::AwaitingChildDecision`]: the
            // child's decision has not yet reached the registry.
            state.records[index].steer_tickets.push(SteerTicket {
                guidance_id,
                phase: SteerTicketPhase::AwaitingChildDecision,
            });
            Ok((
                guidance_id,
                receiver,
                GuidanceTicket {
                    state: Arc::clone(&self.state),
                    subagent_id: subagent_id.clone(),
                    guidance_id,
                    write_started,
                },
            ))
        }
    }
}
