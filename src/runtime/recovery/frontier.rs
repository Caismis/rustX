//! The persisted recovery frontier.
//!
//! The frontier is the result of folding the Event Journal from its first
//! fact through one exact sequence. The Journal append transaction advances
//! it atomically with the fact it covers, so a reopened conversation folds
//! only the facts after it (normally none) instead of its whole history.
//!
//! Its size is bounded by concurrently open work, never by history: closed
//! attempts, settled tool repairs, published background executions and
//! terminal subagents leave it. Known tool outcomes are retained as the
//! sequence of their durable outcome fact, not as a copy of the result, so
//! the frontier never duplicates tool output.
//!
//! The frontier names the Journal row it covers (`through`, `through_event`)
//! and the rows of every open item it references; recovery verifies those
//! rows before trusting it and fails closed when they disagree.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::{
    AttemptEvidence, BackgroundEvidence, ExternalRequestLifecycle, RequestOutcome,
    SubagentEvidence, ToolExternalSummary,
};
use crate::durable::store::canonical::CanonicalToolResultOwner;
use crate::events::types::{AttemptFailure, RuntimeEvent, RuntimeEventEnvelope};
use crate::runtime::identity::{
    AttemptId, ConversationId, EventId, MessageId, RequestId, SubagentId, ToolCallId,
    ToolExecutionId, ToolId,
};
use crate::runtime::types::RuntimeError;

/// One non-terminal attempt and the Journal row that opened it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct FrontierAttempt {
    /// The sequence of the attempt's `AttemptStarted` fact.
    pub(super) started: u64,
    pub(super) evidence: AttemptEvidence,
}

/// One tool call whose canonical `ToolResult` may still need repair.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct FrontierRepair {
    pub(super) attempt: AttemptId,
    pub(super) call: ToolCallId,
    pub(super) tool_id: ToolId,
    /// The sequence of the durable outcome fact, or `None` while the started
    /// call's external outcome is unknown.
    pub(super) outcome: Option<u64>,
}

/// The Journal fold through [`RecoveryFrontier::through`].
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryFrontier {
    /// The last folded Journal sequence; zero before the first fact.
    pub(super) through: u64,
    /// The identity of the Journal row at `through`.
    pub(super) through_event: Option<EventId>,
    pub(super) attempts: BTreeMap<AttemptId, FrontierAttempt>,
    /// Ordered by owning attempt, then call id.
    pub(super) repairs: Vec<FrontierRepair>,
    pub(super) background: BTreeMap<ToolExecutionId, BackgroundEvidence>,
    pub(super) subagents: BTreeMap<SubagentId, SubagentEvidence>,
    pub(super) highest_attempt_ordinal: Option<u64>,
    pub(super) highest_subagent_ordinal: u64,
    pub(super) saw_any_attempt: bool,
    pub(super) unanswered_adopted_turn: Option<(MessageId, Option<crate::goal::GoalRef>)>,
    pub(super) unadopted_goal_round: Option<(MessageId, crate::goal::GoalRef)>,
    /// Latest primary request and the Journal row that committed it.
    /// Compact private keys keep settled frontiers in the store root page.
    #[serde(rename = "primary", skip_serializing_if = "Option::is_none")]
    pub(super) latest_primary_request: Option<PrimaryRequestPointer>,
    /// Exact unresolved adoption task. Latest Human wins; the first delegated
    /// input is kept only while no task has been selected. Notice-only batches
    /// do not erase it. Cleared only when the obligation is consumed.
    #[serde(rename = "task", skip_serializing_if = "Option::is_none")]
    pub(super) continuation_task: Option<ContinuationTaskPointer>,
}

/// Exact request identity and the Journal row that committed its start.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PrimaryRequestPointer {
    #[serde(rename = "id")]
    pub(super) request_id: RequestId,
    #[serde(rename = "at")]
    pub(super) source_sequence: u64,
}

/// Which canonical provenance established the current task winner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum TaskProvenanceKind {
    Human,
    Delegated,
}

/// Journal row and canonical identity of the unresolved adoption task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ContinuationTaskPointer {
    #[serde(rename = "id")]
    pub(super) message_id: MessageId,
    pub(super) kind: TaskProvenanceKind,
    /// Sequence of the `InboundTurnAdopted` fact that established this winner.
    #[serde(rename = "at")]
    pub(super) source_sequence: u64,
}

pub(super) fn message_task_kind(
    message: &crate::message::types::MessageBlock,
) -> Option<TaskProvenanceKind> {
    use crate::message::types::{InboundKind, MessageBlock, UserSource};
    match message {
        MessageBlock::User(message) if message.kind == InboundKind::Message => match message.source
        {
            UserSource::Human => Some(TaskProvenanceKind::Human),
            UserSource::Agent { .. } => Some(TaskProvenanceKind::Delegated),
            _ => None,
        },
        _ => None,
    }
}

/// Latest Human in one batch, otherwise its first delegated input.
fn batch_task(
    adopted: &[crate::message::types::MessageBlock],
) -> Option<(MessageId, TaskProvenanceKind)> {
    adopted
        .iter()
        .rev()
        .find_map(|message| {
            (message_task_kind(message) == Some(TaskProvenanceKind::Human)).then(|| {
                (
                    crate::conversation::message_id_of(message),
                    TaskProvenanceKind::Human,
                )
            })
        })
        .or_else(|| {
            adopted.iter().find_map(|message| {
                (message_task_kind(message) == Some(TaskProvenanceKind::Delegated)).then(|| {
                    (
                        crate::conversation::message_id_of(message),
                        TaskProvenanceKind::Delegated,
                    )
                })
            })
        })
}

impl RecoveryFrontier {
    /// The last Journal sequence this frontier covers.
    #[must_use]
    pub fn through(&self) -> u64 {
        self.through
    }

    /// The identity of the Journal row at [`Self::through`].
    #[must_use]
    pub fn through_event(&self) -> Option<&EventId> {
        self.through_event.as_ref()
    }

    /// Folds one unresolved adoption batch into the constant-size task winner.
    ///
    /// A Human in this batch replaces any earlier winner. A delegated input is
    /// recorded only while no task has been selected, and only the first such
    /// input. A notice-only batch leaves the current winner unchanged.
    pub fn reduce_adoption_task(
        &mut self,
        source_sequence: u64,
        adopted: &[crate::message::types::MessageBlock],
    ) {
        let Some((message_id, kind)) = batch_task(adopted) else {
            return;
        };
        if kind == TaskProvenanceKind::Delegated && self.continuation_task.is_some() {
            return;
        }
        self.continuation_task = Some(ContinuationTaskPointer {
            message_id,
            kind,
            source_sequence,
        });
    }

    /// Encodes the frontier for its durable column.
    ///
    /// # Errors
    ///
    /// Returns the encoder failure.
    pub fn encode(&self) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec(self)
    }

    /// Decodes a durable frontier.
    ///
    /// # Errors
    ///
    /// Returns the decoder failure for a malformed or unknown frontier shape.
    pub fn decode(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        serde_json::from_slice(bytes)
    }

    /// The canonical `ToolResult` whose primary owner a fold of `envelope`
    /// needs: only a recovery repair commit carries no attempt identity.
    #[must_use]
    pub fn owner_query(envelope: &RuntimeEventEnvelope) -> Option<(&MessageId, &ToolCallId)> {
        match (&envelope.attempt_id, &envelope.event) {
            (
                None,
                RuntimeEvent::ToolMessageCommitted {
                    message_id,
                    tool_call_id,
                },
            ) => Some((message_id, tool_call_id)),
            _ => None,
        }
    }

    fn repair_index(&self, attempt: &AttemptId, call: &ToolCallId) -> Result<usize, usize> {
        self.repairs
            .binary_search_by(|entry| (&entry.attempt, &entry.call).cmp(&(attempt, call)))
    }

    fn set_repair(&mut self, repair: FrontierRepair) {
        match self.repair_index(&repair.attempt, &repair.call) {
            Ok(index) => self.repairs[index] = repair,
            Err(index) => self.repairs.insert(index, repair),
        }
    }

    fn remove_repair(&mut self, attempt: &AttemptId, call: &ToolCallId) -> Option<FrontierRepair> {
        self.repair_index(attempt, call)
            .ok()
            .map(|index| self.repairs.remove(index))
    }

    /// Records that this attempt identity entered durable authority.
    fn note_attempt(&mut self, conversation: &ConversationId, attempt_id: &AttemptId) {
        self.saw_any_attempt = true;
        if let Some(ordinal) = attempt_id.conversation_ordinal(conversation) {
            self.highest_attempt_ordinal = Some(
                self.highest_attempt_ordinal
                    .map_or(ordinal, |seen| seen.max(ordinal)),
            );
        }
    }

    fn current_attempt_mut(
        &mut self,
        conversation: &ConversationId,
        envelope: &RuntimeEventEnvelope,
    ) -> Option<&mut AttemptEvidence> {
        let attempt_id = envelope.attempt_id.clone()?;
        self.note_attempt(conversation, &attempt_id);
        self.attempts
            .get_mut(&attempt_id)
            .map(|attempt| &mut attempt.evidence)
    }

    /// Records a durably known tool outcome in the owning attempt's external
    /// summary and recomputes the unknown state against the outstanding
    /// repair evidence.
    ///
    /// A durable outcome makes exactly one call known; whether the attempt
    /// still has an unknown outcome is recomputed from the repair map (any
    /// other started call without a durable outcome), with the irreversible
    /// state preserved — a call whose unknown outcome was already canonically
    /// committed can never become known.
    fn note_known_tool_outcome(&mut self, attempt_id: &AttemptId) {
        let outstanding_unknown = self
            .repairs
            .iter()
            .any(|repair| repair.attempt == *attempt_id && repair.outcome.is_none());
        let Some(attempt) = self.attempts.get_mut(attempt_id) else {
            return;
        };
        if attempt.evidence.tools == ToolExternalSummary::UnknownIrreversible {
            return;
        }
        attempt.evidence.tools = if outstanding_unknown {
            ToolExternalSummary::UnknownOutstanding
        } else {
            ToolExternalSummary::AllOutcomesKnown
        };
    }

    /// Whether this attempt terminal was a **decision about the turn**, and
    /// therefore consumes the durable answer obligation.
    ///
    /// Every terminal a live runtime commits is one: completion, cancellation,
    /// timeout, limit exhaustion, and an ordinary runtime or model failure all
    /// mean the runtime finished with the turn, answered or not.
    ///
    /// [`RuntimeError::RestartInterrupted`] is the one terminal no live
    /// runtime ever commits. Recovery writes it, about an attempt whose
    /// process-local execution state is gone, and it is a statement about the
    /// *attempt* rather than about the *turn*: the turn's canonical
    /// `UserMessage` is untouched and nothing was ever sent for it. The
    /// obligation therefore survives it and transfers to whichever attempt
    /// continues the turn next, until a `ModelRequestStarted` finally carries
    /// it to the provider or a decided terminal concludes it.
    ///
    /// Without this, the recovery reconciliation that terminalizes the
    /// interrupted attempt would durably erase the very obligation the same
    /// recovery reported as continuable, and the turn would be stranded by the
    /// next process death before its successor attempt reached a request
    /// start.
    fn decides_the_turn(event: &RuntimeEvent) -> bool {
        !matches!(
            event,
            RuntimeEvent::AttemptFailed {
                error: AttemptFailure::Runtime {
                    error: RuntimeError::RestartInterrupted { .. }
                },
                ..
            }
        )
    }

    /// Folds one durable event into the unresolved working set.
    ///
    /// `owner` is the verified primary owner of the canonical `ToolResult` named
    /// by [`Self::owner_query`], when that query applies and the result has
    /// one. The caller must fold contiguous Journal facts in order.
    #[allow(clippy::too_many_lines)] // One event vocabulary, one fold, one place.
    pub fn fold(
        &mut self,
        conversation: &ConversationId,
        envelope: &RuntimeEventEnvelope,
        owner: Option<&CanonicalToolResultOwner>,
    ) {
        self.through = envelope.sequence;
        self.through_event = Some(envelope.event_id.clone());
        match &envelope.event {
            RuntimeEvent::AttemptStarted { attempt_id } => {
                self.note_attempt(conversation, attempt_id);
                self.attempts
                    .entry(attempt_id.clone())
                    .or_insert(FrontierAttempt {
                        started: envelope.sequence,
                        evidence: AttemptEvidence {
                            request: ExternalRequestLifecycle::NeverStarted,
                            last_started_request: None,
                            tools: ToolExternalSummary::default(),
                            last_accepted_assistant: None,
                        },
                    });
            }
            RuntimeEvent::Goal {
                fact:
                    crate::goal::GoalFact::RoundAdmitted {
                        message_id,
                        current,
                        ..
                    },
            } => {
                self.unadopted_goal_round = Some((message_id.clone(), current.clone()));
            }
            RuntimeEvent::InboundTurnAdopted { message_ids } => {
                // The adoption transaction opened an answer obligation. Only
                // the trailing identity and any exactly correlated Goal
                // authority are retained, never the batch.
                if let Some(last) = message_ids.last() {
                    let goal = self
                        .unadopted_goal_round
                        .as_ref()
                        .and_then(|(id, goal)| message_ids.contains(id).then(|| goal.clone()));
                    if goal.is_some() {
                        self.unadopted_goal_round = None;
                    }
                    self.unanswered_adopted_turn = Some((last.clone(), goal));
                }
                // Adoption is the durable boundary of a new logical model
                // step. An existing attempt may have already completed a
                // previous request and then accepted fresh inbound at a safe
                // boundary; that previous request must not be used as the
                // carryover selector input if this process dies before the
                // new step crosses model start.
                for attempt in self.attempts.values_mut() {
                    attempt.evidence.last_started_request = None;
                }
            }
            RuntimeEvent::AttemptCompleted { attempt_id, .. }
            | RuntimeEvent::AttemptCancelled { attempt_id, .. }
            | RuntimeEvent::AttemptTimedOut { attempt_id }
            | RuntimeEvent::AttemptLimitExceeded { attempt_id, .. }
            | RuntimeEvent::AttemptFailed { attempt_id, .. } => {
                self.note_attempt(conversation, attempt_id);
                // A **decided** terminal concluded the turn the attempt
                // owned. A live runtime starts nothing further for it — a
                // cancelled turn stays unanswered — so the obligation is
                // consumed here exactly as it is live.
                //
                // A recovery-generated terminal is not such a decision (see
                // [`Self::decides_the_turn`]): it records that a process
                // died, and the very recovery that writes it is the one still
                // permitting the turn to continue. Consuming the obligation
                // there would let recovery destroy its own permission, and a
                // second death before the successor attempt reached its
                // request start would strand the turn forever.
                if Self::decides_the_turn(&envelope.event) {
                    self.unanswered_adopted_turn = None;
                    self.continuation_task = None;
                }
                // A durable terminal is absorbing: the attempt leaves the
                // unresolved working set and never returns to it. Its tool
                // repair evidence survives independently: an incomplete
                // canonical turn that outlived its owning attempt (Class D)
                // must stay repairable from its durable outcome, and entries
                // are released only by their own `ToolMessageCommitted`.
                self.attempts.remove(attempt_id);
            }
            RuntimeEvent::ModelRequestStarted {
                owner: crate::model::snapshot::ModelRequestOwner::Agent { .. },
                request_id,
                ..
            } => {
                // Survives attempt settlement. Only a newer primary start replaces it.
                self.latest_primary_request = Some(PrimaryRequestPointer {
                    request_id: request_id.clone(),
                    source_sequence: envelope.sequence,
                });
                // The adopted turn was carried to the provider. From here the
                // external-outcome plane owns it: whether it is answered,
                // indeterminate, or known-and-unsettled is the attempt
                // classification's question, never the obligation's.
                self.unanswered_adopted_turn = None;
                self.continuation_task = None;
                if let Some(attempt) = self.current_attempt_mut(conversation, envelope) {
                    // The newest start is the in-flight request. The
                    // transition is monotonic: a started request can never
                    // become "never started" again.
                    attempt.request = ExternalRequestLifecycle::StartedOutcomeUnknown {
                        request_id: request_id.clone(),
                    };
                    attempt.last_started_request = Some(request_id.clone());
                }
            }
            RuntimeEvent::ModelRequestCompleted {
                owner: crate::model::snapshot::ModelRequestOwner::Agent { .. },
                ..
            }
            | RuntimeEvent::ModelRequestFailed {
                owner: crate::model::snapshot::ModelRequestOwner::Agent { .. },
                ..
            } => {
                if let Some(attempt) = self.current_attempt_mut(conversation, envelope) {
                    let completed =
                        matches!(&envelope.event, RuntimeEvent::ModelRequestCompleted { .. });
                    let outcome = if completed {
                        RequestOutcome::Completed
                    } else {
                        RequestOutcome::Failed
                    };
                    // The provider outcome is durably known, whatever it
                    // was. This transition is **monotonic**: the known
                    // outcome is never converted back into "never started".
                    attempt.request = match &attempt.request {
                        ExternalRequestLifecycle::StartedOutcomeUnknown { request_id } => {
                            ExternalRequestLifecycle::StartedOutcomeKnown {
                                request_id: Some(request_id.clone()),
                                outcome,
                            }
                        }
                        ExternalRequestLifecycle::StartedOutcomeKnown {
                            request_id,
                            outcome: _,
                        } => ExternalRequestLifecycle::StartedOutcomeKnown {
                            request_id: request_id.clone(),
                            outcome,
                        },
                        // A durable outcome with no start fact is a journal
                        // anomaly; the outcome is still durably known and
                        // still proves external work happened.
                        ExternalRequestLifecycle::NeverStarted => {
                            ExternalRequestLifecycle::StartedOutcomeKnown {
                                request_id: None,
                                outcome,
                            }
                        }
                    };
                }
            }
            RuntimeEvent::AssistantMessageCommitted { message_id } => {
                // The owning attempt of an unresolved Assistant is read from
                // its native canonical attribution at recovery; the fold
                // retains only the latest acceptance per open attempt.
                if let Some(attempt) = envelope.attempt_id.as_ref()
                    && let Some(open) = self.attempts.get_mut(attempt)
                {
                    open.evidence.last_accepted_assistant = Some(message_id.clone());
                }
            }
            RuntimeEvent::ToolExecutionStarted {
                tool_call_id,
                tool_id,
            } => {
                if let Some(attempt) = envelope.attempt_id.clone() {
                    // Attempt summary: external tool execution happened, and
                    // this call's external outcome is unknown until a durable
                    // outcome fact follows.
                    if let Some(open) = self.attempts.get_mut(&attempt) {
                        open.evidence.tools = match open.evidence.tools {
                            // A new unknown call makes the summary unknown.
                            ToolExternalSummary::NeverStarted
                            | ToolExternalSummary::AllOutcomesKnown => {
                                ToolExternalSummary::UnknownOutstanding
                            }
                            // Already unknown (outstanding or irreversible):
                            // one more started call changes nothing.
                            other => other,
                        };
                    }
                    // Per-call repair evidence: the call needs its missing
                    // canonical `ToolResult`, which only the exact durable
                    // outcome (or the honest unknown) can produce.
                    self.set_repair(FrontierRepair {
                        attempt,
                        call: tool_call_id.clone(),
                        tool_id: tool_id.clone(),
                        outcome: None,
                    });
                }
            }
            RuntimeEvent::ToolExecutionCompleted {
                tool_call_id,
                tool_id,
                ..
            }
            | RuntimeEvent::ToolExecutionFailed {
                tool_call_id,
                tool_id,
                ..
            } => {
                if let Some(attempt) = envelope.attempt_id.clone() {
                    self.set_repair(FrontierRepair {
                        attempt: attempt.clone(),
                        call: tool_call_id.clone(),
                        tool_id: tool_id.clone(),
                        outcome: Some(envelope.sequence),
                    });
                    self.note_known_tool_outcome(&attempt);
                }
            }
            RuntimeEvent::ToolMessageCommitted {
                message_id,
                tool_call_id,
            } => {
                // Canonical repair state and attempt external-history summary
                // are separate axes, owned separately. A committed `ToolResult`
                // means the Surface no longer needs this repair: the per-call
                // repair entry is released here and now, whatever the owning
                // attempt's terminal state. The attempt summary independently
                // keeps proving the historical external execution; releasing
                // the detailed entry must never erase it.
                let owning = if let Some(attempt) = &envelope.attempt_id {
                    // A live commit names its owning attempt exactly.
                    Some(attempt.clone())
                } else {
                    // A recovery-generated repair commit carries no attempt
                    // identity. The recovery message identity is
                    // "{assistant_id}-recovered-tool-{call_id}", so the
                    // owning attempt resolves through the exact native owner
                    // of that Assistant — never through a bare call-id scan
                    // that could mark a historical leftover.
                    owner
                        .filter(|owner| {
                            message_id.as_str()
                                == format!(
                                    "{}-recovered-tool-{tool_call_id}",
                                    owner.assistant_message_id
                                )
                        })
                        .map(|owner| owner.attempt_id.clone())
                };
                if let Some(attempt) = owning {
                    // A call whose external outcome was still unknown when its
                    // canonical result committed has an outcome that can never
                    // become durably known: the recovery `OutcomeUnknown`
                    // shape is the canonical representation of "still
                    // unknowable", not evidence of a known outcome. The owning
                    // non-terminal attempt therefore keeps an unknown external
                    // outcome until it terminalizes.
                    let released = self.remove_repair(&attempt, tool_call_id);
                    let was_unknown = released.is_some_and(|repair| repair.outcome.is_none());
                    if was_unknown && let Some(open) = self.attempts.get_mut(&attempt) {
                        // The released call's external outcome can never
                        // become durably known.
                        open.evidence.tools = ToolExternalSummary::UnknownIrreversible;
                    }
                }
            }
            RuntimeEvent::BackgroundExecutionCommitted {
                execution_id,
                tool_call_id,
                tool_id,
                tool_name,
            } => {
                self.background.insert(
                    execution_id.clone(),
                    BackgroundEvidence {
                        sequence: envelope.sequence,
                        execution_id: execution_id.clone(),
                        tool_call_id: tool_call_id.clone(),
                        tool_id: tool_id.clone(),
                        tool_name: tool_name.clone(),
                    },
                );
            }
            RuntimeEvent::BackgroundTerminalPublished { execution_id, .. } => {
                // The terminal publication is absorbing.
                self.background.remove(execution_id);
            }
            RuntimeEvent::AgentActivationAdmission { activation_id, .. } => {
                if let Some(ordinal) = activation_id.conversation_ordinal(conversation) {
                    self.highest_subagent_ordinal = self.highest_subagent_ordinal.max(ordinal);
                }
            }
            RuntimeEvent::SubagentOwnershipCommitted {
                admitted_authority: _,
                parent_agent_id: _,
                subagent_id,
                child_agent_id,
                child_conversation_id,
                origin,
                agent,
                definition_digest,
                profile_digest,
                ownership,
                workspace,
            } => {
                if let Some(ordinal) = subagent_id.conversation_ordinal(conversation) {
                    self.highest_subagent_ordinal = self.highest_subagent_ordinal.max(ordinal);
                }
                self.subagents.insert(
                    subagent_id.clone(),
                    SubagentEvidence {
                        subagent_id: subagent_id.clone(),
                        child_agent_id: child_agent_id.clone(),
                        child_conversation_id: child_conversation_id.clone(),
                        origin: origin.clone(),
                        agent: agent.clone(),
                        definition_digest: definition_digest.clone(),
                        profile_digest: profile_digest.clone(),
                        ownership: *ownership,
                        started_at: envelope.timestamp,
                        workspace: workspace.clone(),
                    },
                );
            }
            RuntimeEvent::SubagentTerminalPublished { subagent_id, .. }
            | RuntimeEvent::SubagentTerminalSettled { subagent_id, .. } => {
                if let Some(ordinal) = subagent_id.conversation_ordinal(conversation) {
                    self.highest_subagent_ordinal = self.highest_subagent_ordinal.max(ordinal);
                }
                // The terminal fact is absorbing: it closes the native child
                // lifecycle. A Workflow terminal settlement never creates a
                // parent notification and the Workflow itself is never
                // replayed.
                self.subagents.remove(subagent_id);
            }
            // Every remaining fact contributes its attempt identity watermark
            // and nothing else.
            //
            // The interaction audit facts (Issue #109) land here deliberately,
            // and their absence from every arm above *is* the invariant: they
            // are evidence, never authority. Recovery reconstructs no pending
            // waiter, republishes no prompt, and never treats a historical
            // `InteractionSettled(Approved)` as permission to run the tool it
            // referred to. A call whose `ToolExecutionStarted` is absent is
            // simply a call that never started, exactly as if no approval had
            // ever been asked for.
            _ => {
                if let Some(attempt_id) = envelope.attempt_id.as_ref() {
                    self.note_attempt(conversation, attempt_id);
                }
            }
        }
    }
}
