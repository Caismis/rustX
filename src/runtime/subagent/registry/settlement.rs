//! Terminal settlement: physical outcome -> terminal candidate -> exactly-once
//! durable terminal publication (with bounded retry and abandonment) ->
//! capacity release.

use super::super::MAX_RESULT_CONTENT_BYTES;
use super::super::SubagentTerminalState;
use super::super::bound_utf8;
use super::super::process::PhysicalOutcome;
use super::super::process::PhysicalSettlement;
use super::super::terminal_publication;
use super::super::terminal_settlement;
use super::super::workflow_output_event;
use super::NotificationState;
use super::SubagentLifecycle;
use super::SubagentRecord;
use super::SubagentRegistry;
use super::SubagentTerminalMode;
use super::TerminalCandidate;
use super::TerminalState;
use super::observation::publish_committed_snapshot;
use super::observation::publish_snapshot;
use crate::events::types::SubagentOwnershipKind;
use crate::runtime::identity::SubagentId;
use crate::runtime::types::CancellationReason;
use std::sync::Arc;
use std::sync::PoisonError;

/// Ownership of one immutable terminal append across the unlocked I/O phase.
struct TerminalPublicationClaim {
    registry: SubagentRegistry,
    activation: SubagentId,
}

impl Drop for TerminalPublicationClaim {
    fn drop(&mut self) {
        self.registry
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .terminal_inflight
            .remove(&self.activation);
    }
}

/// Revalidates a Workflow terminal candidate at the parent boundary before
/// it is durably accepted. The child already validates against its frozen
/// latch, but the parent owns the cross-process result and must not commit a
/// malformed or schema-invalid value merely because a peer sent a successful
/// frame.
fn validate_workflow_candidate(
    terminal: &SubagentTerminalMode,
    candidate: TerminalCandidate,
) -> TerminalCandidate {
    let SubagentTerminalMode::WorkflowOutput { output_schema, .. } = terminal else {
        return candidate;
    };
    if candidate.state != TerminalState::Succeeded {
        return candidate;
    }
    let value = candidate
        .content
        .as_deref()
        .ok_or_else(|| "the Workflow Agent returned no terminal value".to_owned())
        .and_then(|content| {
            serde_json::from_str::<serde_json::Value>(content)
                .map_err(|error| format!("the Workflow Agent terminal value was not JSON: {error}"))
        })
        .and_then(|value| {
            let validator = jsonschema::Validator::new(output_schema)
                .map_err(|error| format!("the Workflow Agent output schema is invalid: {error}"))?;
            if validator.is_valid(&value) {
                Ok(value)
            } else {
                Err(
                    "the Workflow Agent terminal value violated its frozen output schema"
                        .to_owned(),
                )
            }
        });
    match value {
        Ok(value) => TerminalCandidate {
            workflow_value: Some(value),
            ..candidate
        },
        Err(diagnostic) => TerminalCandidate {
            physical_settlement_proven: false,
            state: TerminalState::Failed,
            content: None,
            workflow_value: None,
            diagnostic: Some(bound_utf8(diagnostic, MAX_RESULT_CONTENT_BYTES)),
            reason: None,
            ..candidate
        },
    }
}

/// Builds the bounded content blocks of a terminal publication.
///
/// A successful report is the child's own final answer, child-authored and
/// byte-for-byte what the child emitted: no runtime annotation is ever
/// concatenated into it (Issue #192). Every other terminal is a
/// runtime-authored notice.
fn terminal_blocks(
    record: &SubagentRecord,
    candidate: &TerminalCandidate,
) -> Vec<crate::message::types::UserContentBlock> {
    let text = match candidate.state {
        TerminalState::Succeeded => candidate.content.clone().unwrap_or_default(),
        TerminalState::Failed => format!(
            "Subagent {} (agent {}) failed: {}.",
            record.subagent_id,
            record.agent,
            candidate
                .diagnostic
                .clone()
                .unwrap_or_else(|| "unknown failure".to_owned()),
        ),
        TerminalState::Cancelled => format!(
            "Subagent {} (agent {}) was cancelled ({}).",
            record.subagent_id,
            record.agent,
            candidate.reason.map_or("cancelled", reason_text),
        ),
        TerminalState::Interrupted => format!(
            "Subagent {} (agent {}) was interrupted: its actual outcome is unknown and it was not restarted.",
            record.subagent_id, record.agent
        ),
    };
    vec![crate::message::types::UserContentBlock::Text(
        crate::message::content::TextBlock {
            text: bound_utf8(text, MAX_RESULT_CONTENT_BYTES),
        },
    )]
}

/// The runtime-authored terminal notice of a successful normal child
/// (Issue #192): the parent-model correlation projection naming the exact
/// typed execution handle the creation result returned.
///
/// The success report stays purely child-authored; this adjacent
/// `UserSource::Runtime` item commits in the same durable transaction,
/// ordered before the report, so authorship and ordering are both
/// preserved. A failed/cancelled/interrupted terminal is already one
/// runtime-authored message that names the child execution, so it
/// deliberately carries no separate notice.
fn success_terminal_notice(
    record: &SubagentRecord,
    candidate: &TerminalCandidate,
) -> Option<crate::durable::inbox::InboundDraft> {
    (candidate.state == TerminalState::Succeeded).then(|| {
        super::super::terminal_notice(
            &record.child_agent_id,
            &record.subagent_id,
            &record.agent,
            candidate.timestamp,
        )
    })
}

/// Maps the registry's terminal vocabulary onto the durable event's.
const fn candidate_state(candidate: &TerminalCandidate) -> SubagentTerminalState {
    match candidate.state {
        TerminalState::Succeeded => SubagentTerminalState::Succeeded,
        TerminalState::Failed => SubagentTerminalState::Failed,
        TerminalState::Cancelled => SubagentTerminalState::Cancelled,
        TerminalState::Interrupted => SubagentTerminalState::Interrupted,
    }
}

/// Stops the record-owned deadline task after its cancellation/terminal winner
/// has been selected under the registry mutex.
pub(super) fn abort_deadline_task(task: Option<tokio::task::JoinHandle<()>>) {
    if let Some(task) = task {
        task.abort();
    }
}

/// Maps a typed cancellation reason to the parent-facing diagnostic.
const fn reason_text(reason: CancellationReason) -> &'static str {
    match reason {
        CancellationReason::UserRequested => "requested by the user",
        CancellationReason::RuntimeShutdown => "the runtime is shutting down",
        CancellationReason::ParentCancelled => "the parent operation was cancelled",
        CancellationReason::SubagentExecutionDeadlineExceeded => {
            "the subagent execution deadline expired"
        }
    }
}

impl SubagentRegistry {
    /// **Settlement.** Canonicalizes the driver's physical outcome against
    /// the lifecycle, then drives the durable result acceptance.
    ///
    /// Cancellation intent is canonical once its typed frame is delivered:
    /// a physical loss caused by that cancellation settles as cancelled with
    /// the registry's committed reason. An unexpected loss with proven
    /// physical settlement is Interrupted; a low-level control or
    /// containment failure stays an explicit Failed infrastructure
    /// outcome. The durable compound transaction makes the publication
    /// exactly-once.
    #[allow(clippy::too_many_lines)] // one coherent physical-to-durable settlement pipeline
    pub(super) fn settle_from_driver(
        &self,
        subagent_id: &SubagentId,
        settlement: PhysicalSettlement,
    ) {
        // Child death is the end of interaction actionability even when the
        // parent mailbox has already entered a degraded/draining state and
        // cannot accept another lifecycle publication. Remove presentation
        // entries first; the terminal lifecycle snapshot below is separate.
        self.remove_child_interactions(subagent_id);
        if self.config.mailbox.begin_settlement_admission().is_err() {
            return;
        }
        let PhysicalSettlement {
            outcome,
            nested,
            runtime_root_cleanup_error,
        } = settlement;
        // An unproven nested settlement or failed exact-root cleanup is a
        // terminal classification input, not an ignorable warning. A
        // successful semantic frame cannot become a parent-facing success
        // until every required physical boundary is proven settled.
        let settlement_diagnostic = [
            nested.unproven_diagnostic(),
            runtime_root_cleanup_error
                .as_ref()
                .map(|detail| format!("the child physical runtime root was not removed: {detail}")),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
        let settlement_diagnostic =
            (!settlement_diagnostic.is_empty()).then_some(settlement_diagnostic.join("; "));
        let physical_settlement_unproven =
            matches!(&outcome, PhysicalOutcome::ControlFailure { .. })
                || !nested.unproven.is_empty()
                || runtime_root_cleanup_error.is_some();
        // Test-only: park the terminal settlement path after every physical
        // boundary (nested containment, runtime root)
        // is settled and immediately before the registry mutex that creates
        // the terminal candidate and commits `... -> PublishingTerminal`.
        // The pause holds no registry lock: while the terminal authority
        // contender is parked here, a concurrent deadline can race the exact
        // authority boundary and commit `Running -> Cancelling` first; the
        // released settlement then observes that intent and must honor it.
        #[cfg(test)]
        let terminal_authority_hook = {
            let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            state.terminal_authority_hook.clone()
        };
        #[cfg(test)]
        if let Some(hook) = terminal_authority_hook {
            hook.wait();
        }
        let candidate = {
            let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            let Some(&index) = state.index.get(subagent_id) else {
                return;
            };
            if state.records[index].lifecycle.is_terminal()
                || state.records[index].publication_abandoned
            {
                return;
            }
            // An unproven settlement needs no separate Agent mark: the
            // terminal activation's own unproven physical fact keeps its
            // durable Agent Unavailable until native proof arrives.
            let record = &mut state.records[index];
            // Terminal candidate creation and timer invalidation share this
            // registry mutex. A deadline that has not already committed
            // cancellation is stopped before this child can publish any
            // terminal fact; a timer that acquired the mutex first has
            // already made the record Cancelling and is reflected below.
            let deadline_task = record.deadline_task.take();
            // Lifecycle is the terminal truth; activity is live-only. Every
            // terminal settlement resets the projection to neutral (with a
            // bumped revision, so the reset itself is observable) while
            // keeping the counters and last-activity timestamp as the final
            // record of what the child did.
            record.observation.settle_neutral();
            let cancelling = matches!(record.lifecycle, SubagentLifecycle::Cancelling);
            let workflow_output = record.ownership == SubagentOwnershipKind::Workflow;
            // The publication timestamp freezes at canonicalization: every
            // later bounded retry rebuilds the byte-identical draft, so an
            // ambiguous commit resolves as the idempotent correlation
            // retry, never a conflict.
            let timestamp = self.config.clock.now();
            let candidate = match outcome {
                PhysicalOutcome::Completed(frame) => {
                    match (workflow_output, cancelling, frame.status) {
                        // A Workflow Agent can report `Succeeded` only after the
                        // child Agent Loop has committed the reserved
                        // `workflow_output` latch. That frame is the
                        // cross-process observation of the output/cancellation
                        // linearization point, so a cancellation request that
                        // arrived while the frame was in flight cannot rewrite
                        // the committed value.
                        (true, _, super::super::ipc::ChildResultStatus::Succeeded)
                        | (false, false, super::super::ipc::ChildResultStatus::Succeeded) => {
                            TerminalCandidate {
                                physical_settlement_proven: false,
                                state: TerminalState::Succeeded,
                                content: Some(bound_utf8(
                                    frame.content.unwrap_or_default(),
                                    MAX_RESULT_CONTENT_BYTES,
                                )),
                                workflow_value: None,
                                diagnostic: None,
                                reason: None,
                                timestamp,
                            }
                        }
                        (_, false, super::super::ipc::ChildResultStatus::Failed) => {
                            TerminalCandidate {
                                physical_settlement_proven: false,
                                state: TerminalState::Failed,
                                content: None,
                                workflow_value: None,
                                diagnostic: Some(bound_utf8(
                                    frame
                                        .diagnostic
                                        .unwrap_or_else(|| "the child attempt failed".to_owned()),
                                    MAX_RESULT_CONTENT_BYTES,
                                )),
                                reason: None,
                                timestamp,
                            }
                        }
                        (_, false, super::super::ipc::ChildResultStatus::Cancelled) => {
                            TerminalCandidate {
                                physical_settlement_proven: false,
                                state: TerminalState::Cancelled,
                                content: None,
                                workflow_value: None,
                                diagnostic: None,
                                // A child result without a committed parent
                                // cancellation has no semantic reason in its wire
                                // envelope. Do not fabricate UserRequested.
                                reason: None,
                                timestamp,
                            }
                        }
                        // Cancellation intent is canonical: a completed frame
                        // after the intent settles as cancelled.
                        (_, true, _) => TerminalCandidate {
                            physical_settlement_proven: false,
                            state: TerminalState::Cancelled,
                            content: None,
                            workflow_value: None,
                            diagnostic: None,
                            reason: record.cancel_reason,
                            timestamp,
                        },
                    }
                }
                PhysicalOutcome::Lost {
                    diagnostic,
                    cancellation_delivered,
                    ..
                } => {
                    if physical_settlement_unproven {
                        // A missing semantic result plus an unproven
                        // containment/cleanup boundary is an explicit
                        // infrastructure failure, never a clean
                        // Interrupted state.
                        TerminalCandidate {
                            physical_settlement_proven: false,
                            state: TerminalState::Failed,
                            content: None,
                            workflow_value: None,
                            diagnostic: Some(bound_utf8(diagnostic, MAX_RESULT_CONTENT_BYTES)),
                            reason: None,
                            timestamp,
                        }
                    } else if cancelling && cancellation_delivered {
                        // The child died after the registry's committed
                        // cancellation was delivered. This includes driver
                        // escalation: physical death cannot erase the
                        // logical cancellation cause.
                        TerminalCandidate {
                            physical_settlement_proven: false,
                            state: TerminalState::Cancelled,
                            content: None,
                            workflow_value: None,
                            diagnostic: None,
                            reason: record.cancel_reason,
                            timestamp,
                        }
                    } else {
                        // The direct process/control plane settled, but no
                        // valid semantic terminal arrived. The outcome is
                        // unknown, not a known model failure.
                        TerminalCandidate {
                            physical_settlement_proven: false,
                            state: TerminalState::Interrupted,
                            content: None,
                            workflow_value: None,
                            diagnostic: Some(bound_utf8(diagnostic, MAX_RESULT_CONTENT_BYTES)),
                            reason: None,
                            timestamp,
                        }
                    }
                }
                PhysicalOutcome::ControlFailure { diagnostic } => TerminalCandidate {
                    // A required process/control operation was not proven.
                    // This is an explicit infrastructure failure, including
                    // after a cancellation intent.
                    physical_settlement_proven: false,
                    state: TerminalState::Failed,
                    content: None,
                    workflow_value: None,
                    diagnostic: Some(bound_utf8(diagnostic, MAX_RESULT_CONTENT_BYTES)),
                    reason: None,
                    timestamp,
                },
            };
            let candidate = match settlement_diagnostic {
                None => candidate,
                Some(diagnostic) if physical_settlement_unproven => {
                    // Physical settlement failure is an infrastructure
                    // failure even when the child emitted a semantic success
                    // (or a cancellation frame). Never carry successful
                    // child content across this failed provenance boundary.
                    let diagnostic = match candidate.diagnostic {
                        Some(existing) => format!("{existing}; {diagnostic}"),
                        None => diagnostic,
                    };
                    TerminalCandidate {
                        physical_settlement_proven: false,
                        state: TerminalState::Failed,
                        content: None,
                        workflow_value: None,
                        diagnostic: Some(bound_utf8(
                            format!(
                                "required child physical settlement was not proven: {diagnostic}"
                            ),
                            MAX_RESULT_CONTENT_BYTES,
                        )),
                        reason: None,
                        timestamp: candidate.timestamp,
                    }
                }
                Some(diagnostic) => TerminalCandidate {
                    diagnostic: Some(bound_utf8(
                        match candidate.diagnostic {
                            Some(existing) => format!("{existing}; {diagnostic}"),
                            None => diagnostic,
                        },
                        MAX_RESULT_CONTENT_BYTES,
                    )),
                    ..candidate
                },
            };
            let mut candidate = validate_workflow_candidate(
                record
                    .terminal
                    .as_ref()
                    .expect("a live driver owns its frozen terminal protocol"),
                candidate,
            );
            candidate.physical_settlement_proven = !physical_settlement_unproven;
            record.physical_settlement_proven = candidate.physical_settlement_proven;
            record.pending_terminal = Some(candidate.clone());
            // Terminal authority linearizes under the same registry mutex as
            // cancellation. Once this transition commits, a late deadline
            // or explicit cancel can only observe PublishingTerminal; it
            // cannot create a second cancellation intent or send a driver
            // command after physical settlement has already won.
            record.lifecycle = SubagentLifecycle::PublishingTerminal;
            // The same instant is the steer invalidation commit (Issue
            // #193): every steer ticket admitted but not yet committed is
            // cleared here, under this mutex, in **either** phase — so no
            // terminal record ever retains live steering arbitration state.
            // Clearing a ticket is registry **lifecycle cleanup**: it never
            // erases an acceptance. A steer whose child never answered is
            // refused at its own commit through the child-side refusal its
            // caller observes (the driver dropped the waiter on exit); a
            // steer whose child durably accepted — whether or not that
            // acceptance had already reached the registry and advanced the
            // ticket to `ChildAcceptedPendingParentCommit` — is still
            // reported accepted at its own commit, because the acceptance
            // predates this natural terminal and is carried by the caller
            // into the commit, which this cleanup cannot touch. Cancellation
            // (`SubagentRegistry::cancel`) clears the same vector at its own
            // commit.
            record.steer_tickets.clear();
            // Issue #178: the successful answer content never rides the live
            // observation/control projection. It exists only in the durable
            // terminal publication draft (`terminal_publication`, unchanged);
            // `detail` keeps failure/cancellation diagnostics only. The
            // Workflow output value is the separate live Workflow result
            // channel (`workflow_agent_output`), equally kept out of the
            // snapshot.
            record.detail = candidate.diagnostic.as_deref().map(Arc::from).or_else(|| {
                candidate
                    .reason
                    .map(|reason| Arc::from(reason_text(reason)))
            });
            if candidate.state == TerminalState::Succeeded {
                record
                    .terminal_workflow_value
                    .clone_from(&candidate.workflow_value);
            }
            abort_deadline_task(deadline_task);
            candidate
        };
        self.publish_terminal(subagent_id, &candidate);
    }

    /// Attempts the durable terminal publication; on failure, enters
    /// `PublishingTerminal` and schedules the bounded retry.
    fn publish_terminal(&self, subagent_id: &SubagentId, _candidate: &TerminalCandidate) {
        let Err(initial_failure) = self.retry_terminal_publication(subagent_id) else {
            return;
        };
        // Bounded publication retry; the candidate is stable from
        // pending_terminal.
        let registry = self.clone_for_task();
        let id = subagent_id.clone();
        tokio::spawn(async move {
            let mut diagnostic = initial_failure;
            for _ in 0..2 {
                match registry.retry_terminal_publication(&id) {
                    Ok(true) => return,
                    Ok(false) => {}
                    Err(error) => diagnostic = error,
                }
            }
            // Reporting exhausted terminal durability is a callback into the
            // owning ConversationRuntime. It happens only after the bounded
            // retry budget is spent and never while the registry mutex is
            // held; the candidate remains retained as PublishingTerminal.
            registry.report_terminal_publication_failure(&id, &diagnostic);
        });
    }

    /// One bounded publication retry. Returns whether the terminal is now
    /// durably committed.
    #[allow(clippy::too_many_lines)] // capture, append and publish one immutable terminal
    pub(super) fn retry_terminal_publication(
        &self,
        subagent_id: &SubagentId,
    ) -> Result<bool, String> {
        enum Append {
            Normal(
                Box<(
                    Option<crate::durable::inbox::InboundDraft>,
                    crate::durable::inbox::InboundDraft,
                    crate::events::types::RuntimeEventEnvelope,
                )>,
            ),
            Workflow(
                Box<(
                    crate::events::types::RuntimeEventEnvelope,
                    Option<crate::events::types::RuntimeEventEnvelope>,
                )>,
            ),
        }
        let _settlement = self
            .config
            .mailbox
            .begin_settlement_admission()
            .map_err(|error| format!("terminal settlement admission failed: {error}"))?;
        let (candidate, append) = {
            let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            let Some(&index) = state.index.get(subagent_id) else {
                return Ok(true);
            };
            let record = &state.records[index];
            if record.lifecycle.is_terminal() {
                return Ok(true);
            }
            let Some(candidate) = record.pending_terminal.clone() else {
                return Ok(true);
            };
            if state.terminal_inflight.contains(subagent_id) {
                return Ok(false);
            }
            let append = if record.ownership == SubagentOwnershipKind::Workflow {
                let event = terminal_settlement(
                    &self.config.conversation_id,
                    subagent_id,
                    &record.child_agent_id,
                    candidate_state(&candidate),
                    candidate.physical_settlement_proven,
                    candidate.timestamp,
                );
                let output =
                    if candidate.state == TerminalState::Succeeded {
                        match (candidate.workflow_value.clone(), &record.terminal) {
                            (
                                Some(value),
                                Some(SubagentTerminalMode::WorkflowOutput {
                                    workflow_id,
                                    run_id,
                                    node_id,
                                    ..
                                }),
                            ) => Some(workflow_output_event(
                                &self.config.conversation_id,
                                subagent_id,
                                workflow_id,
                                run_id,
                                node_id,
                                value,
                                candidate.timestamp,
                            )),
                            _ => return Err(
                                "a successful Workflow terminal candidate has no validated value"
                                    .to_owned(),
                            ),
                        }
                    } else {
                        None
                    };
                Append::Workflow(Box::new((event, output)))
            } else {
                let (draft, event) = terminal_publication(
                    &self.config.conversation_id,
                    subagent_id,
                    &record.child_agent_id,
                    candidate_state(&candidate),
                    terminal_blocks(record, &candidate),
                    candidate.physical_settlement_proven,
                    candidate.timestamp,
                );
                Append::Normal(Box::new((
                    success_terminal_notice(record, &candidate),
                    draft,
                    event,
                )))
            };
            // PublishingTerminal pins the immutable candidate; cancellation
            // cannot rewrite it. This explicit claim excludes another append
            // while the registry mutex is released for SQLite.
            state.terminal_inflight.insert(subagent_id.clone());
            (candidate, append)
        };
        let _claim = TerminalPublicationClaim {
            registry: self.clone(),
            activation: subagent_id.clone(),
        };
        crate::runtime::process_death::reach("after:subagent_physical_settlement");
        let result = match append {
            Append::Normal(payload) => {
                let (notice, draft, event) = *payload;
                self.config
                    .mailbox
                    .accept_subagent_terminal(notice, draft, event)
                    .map_err(|error| error.to_string())
            }
            Append::Workflow(payload) => {
                let (event, output) = *payload;
                match output {
                    Some(output) => self
                        .config
                        .mailbox
                        .commit_workflow_agent_terminal(event, output)
                        .map(|(terminal, _)| terminal)
                        .map_err(|error| error.to_string()),
                    None => self
                        .config
                        .mailbox
                        .commit_subagent_terminal(event)
                        .map_err(|error| error.to_string()),
                }
            }
        };
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(&index) = state.index.get(subagent_id) else {
            return Ok(true);
        };
        if state.records[index].lifecycle.is_terminal() {
            return Ok(true);
        }
        debug_assert!(state.terminal_inflight.contains(subagent_id));
        match result {
            Ok(committed) => {
                let record = &mut state.records[index];
                record.lifecycle = match candidate.state {
                    TerminalState::Succeeded => SubagentLifecycle::Succeeded,
                    TerminalState::Failed => SubagentLifecycle::Failed,
                    TerminalState::Cancelled => SubagentLifecycle::Cancelled,
                    TerminalState::Interrupted => SubagentLifecycle::Interrupted,
                };
                record.physical_settlement_proven = candidate.physical_settlement_proven;
                record.pending_terminal = None;
                record.publication_abandoned = false;
                record.notification = if record.ownership == SubagentOwnershipKind::Workflow {
                    NotificationState::None
                } else {
                    NotificationState::Delivered
                };
                publish_committed_snapshot(&mut state, &self.state_version, index, &committed);
                Ok(true)
            }
            Err(error) => {
                state.records[index].notification = NotificationState::Failed;
                publish_snapshot(&mut state, &self.state_version, index);
                Err(error)
            }
        }
    }

    /// Reports an exhausted terminal-publication budget to the owning
    /// runtime and only then exposes the explicit abandoned/unresolved fact.
    /// The failure sink is copied while the registry is locked, but invoked
    /// after that guard is dropped: the lock graph is
    /// `ConversationRuntime -> SubagentRegistry`, never the reverse.
    fn report_terminal_publication_failure(&self, subagent_id: &SubagentId, diagnostic: &str) {
        let Ok(_settlement) = self.config.mailbox.begin_settlement_admission() else {
            return;
        };
        let sink = {
            let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            state.failure_sink.clone()
        };
        if let Some(sink) = sink {
            sink.terminal_publication_failed(subagent_id, diagnostic);
        }
        self.mark_publication_abandoned(subagent_id);
    }

    /// Marks a terminal publication abandoned after the bounded retry.
    fn mark_publication_abandoned(&self, subagent_id: &SubagentId) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(&index) = state.index.get(subagent_id) else {
            return;
        };
        if state.records[index].lifecycle.is_terminal()
            || state.terminal_inflight.contains(subagent_id)
        {
            return;
        }
        state.records[index].publication_abandoned = true;
        publish_snapshot(&mut state, &self.state_version, index);
    }
}
