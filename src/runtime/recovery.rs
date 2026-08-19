//! Durable startup recovery (Issue #12, M9a).
//!
//! One process restart reconstructs **one** coherent conversation from rustX's
//! own durable authority, classifies the crash-time non-terminal work
//! deterministically, reconciles only what can be stated honestly, and permits
//! only the continuation that follows from durable evidence.
//!
//! ```text
//! durable facts
//!     -> reconstruct        (read only; no new durable fact, no external call)
//!     -> classify           (pure; same facts -> same classification)
//!     -> reconcile          (bounded atomic durable transitions)
//!     -> recovered runtime state
//!     -> resume only proven-safe work
//! ```
//!
//! The four phases are distinct types and distinct calls, never one opaque
//! `recover()` whose behavior falls out of incidental control flow:
//!
//! ```text
//! RecoveryEvidence::reconstruct(&store)   phase 1
//! RecoveryPlan::classify(&evidence)       phase 2
//! plan.reconcile(&store, clock)           phase 3
//! RecoveryReport::resume                  phase 4 (a permission, consumed by
//!                                                  ConversationRuntime)
//! ```
//!
//! # The governing invariant
//!
//! > Recovery reconstructs what durably happened. It never invents success,
//! > never silently replays an ambiguous external side effect, and never
//! > regenerates historical request/context from current configuration.
//!
//! and therefore, three times over:
//!
//! ```text
//! exact historical reconstruction  !=  safe replay permission
//! external outcome unknown         !=  retry
//! external outcome known           !=  never externally started
//! ```
//!
//! The evidence model keeps the **external execution lifecycle** and the
//! **canonical structure lifecycle** on separate axes. A committed canonical
//! `ToolResult` means the Surface no longer needs that repair; it never
//! means the historical `ToolExecutionStarted` is forgotten. A durably
//! known provider outcome means the request definitely executed; it never
//! means "nothing started". Only an attempt with **zero** durable
//! external-start evidence — no `ModelRequestStarted`, no
//! `ToolExecutionStarted`, ever — may be classified as the safe Class-B
//! continuation case.
//!
//! # The recovery-prefix invariant
//!
//! > Every successfully committed prefix of recovery reconciliation is
//! > itself a valid, truth-preserving input to a subsequent recovery.
//!
//! Reconciliation commits repair, attempt terminal, and background
//! publication as **separate** atomic transitions on purpose. A crash
//! between any two of them must leave a durable state that the next startup
//! classifies exactly as truthfully as the first did — in particular a
//! `ToolMessageCommitted` committed by a repair must not erase the
//! external-start evidence of the still-non-terminal owning attempt.
//!
//! # Ownership
//!
//! Recovery **policy** belongs to the [`ConversationRuntime`] and consumes
//! [`ConversationStore`] evidence. The store exposes semantic durable facts
//! and semantic transactions; it never decides whether an ambiguous request is
//! safe to replay. Nothing here reads a Runtime Client snapshot, a TUI cache,
//! an attachment state, current Skill discovery, current `models.json`, the
//! current filesystem, or a regenerated dynamic context: current configuration
//! configures **future** work and may never fill a hole in **historical**
//! work.
//!
//! [`ConversationRuntime`]: crate::runtime::conversation_runtime::ConversationRuntime
//!
//! # Bounded working set
//!
//! The evidence fold pages the Event Journal
//! ([`ConversationStore::read_events`]) and retains only the *unresolved*
//! state. Reads are O(history); hot memory is O(unresolved work):
//!
//! ```text
//! recovery hot memory =
//!     O(nonterminal attempt summaries)      (<= 1 by the admission invariant)
//!   + O(canonical tool repairs outstanding) (only while a ToolResult is missing)
//!   + O(unpublished background executions)  (bounded by background policy)
//!   + O(active Surface attribution)         (bounded by the active working set)
//! ```
//!
//! The tool plane is split across two owners on purpose. [`AttemptEvidence`]
//! owns a **bounded summary** of the attempt's foreground-tool external
//! history (did execution happen; is any outcome unknown) that survives the
//! release of every detailed entry — a fully canonicalized tool call never
//! regresses the attempt to "never externally started". The per-call
//! **repair map** owns the exact `ToolExecutionResult` needed to rebuild a
//! missing canonical `ToolResult`, and only while that repair is outstanding:
//! a committed `ToolMessageCommitted` releases the entry, so a long attempt
//! with 10,000 previously settled tool calls retains zero detailed results.
//! A resolved entry is dropped from the fold the moment its resolving fact is
//! read, so complete history is never materialized as `Vec<RuntimeEvent>` or
//! `Vec<RequestSnapshot>`.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};

use crate::conversation::{RecoverySafetyError, recovery_safety};
use crate::durable::{ConversationStore, ConversationStoreError, PendingInboundItem};
use crate::events::types::{AttemptFailure, RuntimeEvent, RuntimeEventEnvelope};
use crate::message::types::{AssistantContentBlock, InboundKind, MessageBlock, ToolMessageBlock};
use crate::runtime::identity::{
    AttemptId, ConversationId, EventId, MessageId, RequestId, SubagentId, ToolCallId,
    ToolExecutionId, ToolId,
};
use crate::runtime::types::{CancellationReason, RuntimeClock, RuntimeError};
use crate::tools::types::{ToolExecutionResult, ToolExecutionStatus};

/// The Event Journal page size of the recovery fold.
///
/// The fold is O(history) in reads and O(unresolved work) in memory; the page
/// size only bounds how much of the journal is decoded at once.
const RECOVERY_PAGE: usize = 256;

/// A startup recovery failure.
///
/// Every variant leaves the conversation runtime unconstructed: a failed
/// recovery never yields a runtime that could admit work as though recovery
/// had completed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecoveryError {
    /// A durable read or a reconciliation transaction failed.
    Durable(String),
    /// The durable state remains incoherent after reconciliation: recovery
    /// could not honestly repair it, so no runtime is produced.
    Unrecoverable(String),
}

impl core::fmt::Display for RecoveryError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Durable(detail) => write!(f, "durable recovery failed: {detail}"),
            Self::Unrecoverable(detail) => {
                write!(f, "the durable conversation cannot be recovered: {detail}")
            }
        }
    }
}

impl std::error::Error for RecoveryError {}

impl From<ConversationStoreError> for RecoveryError {
    fn from(error: ConversationStoreError) -> Self {
        Self::Durable(error.to_string())
    }
}

// ---------------------------------------------------------------------------
// Phase 1 — reconstruct
// ---------------------------------------------------------------------------

/// The durable lifecycle of the model request(s) of one attempt.
///
/// The transition is **monotonic**: `NeverStarted` can move to
/// `StartedOutcomeUnknown` and then to `StartedOutcomeKnown`, but a resolved
/// outcome can never move back to `NeverStarted`. A later turn of the same
/// attempt starts its own request by re-entering `StartedOutcomeUnknown`
/// with the new request identity; the earlier request's durable outcome
/// remains in the Event Journal.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ExternalRequestLifecycle {
    /// No `ModelRequestStarted` ever committed for this attempt. Only this
    /// state is eligible for the Class-B "no external start" continuation.
    NeverStarted,
    /// The request start committed; no durable outcome followed. The
    /// provider may or may not have executed the request.
    StartedOutcomeUnknown {
        /// The in-flight request.
        request_id: RequestId,
    },
    /// The request start committed and the provider outcome is durably
    /// known. This is **never** convertible back to `NeverStarted`.
    ///
    /// The `request_id` is `None` only for the journal anomaly of a durable
    /// outcome with no start fact; the outcome is still durably known and
    /// still proves external work happened.
    StartedOutcomeKnown {
        /// The request, when its start fact committed durably.
        request_id: Option<RequestId>,
        /// What the provider outcome was.
        outcome: RequestOutcome,
    },
}

/// The durably known provider outcome of one model request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestOutcome {
    /// `ModelRequestCompleted` committed durably.
    Completed,
    /// `ModelRequestFailed` committed durably.
    Failed,
}

/// What durable evidence says about one non-terminal attempt.
///
/// The entry exists only while the attempt is unresolved; the attempt's
/// terminal fact removes it from the fold. It owns the classification-relevant
/// external-history summary of the attempt — the model-request lifecycle and
/// the bounded foreground-tool summary — and nothing else: detailed per-call
/// tool results live in the repair map
/// ([`RecoveryEvidence::tool_repairs`]) and are released as soon as the
/// canonical `ToolResult` commits, while this summary survives until the
/// attempt terminalizes.
#[derive(Debug, Clone, PartialEq, Eq)]
struct AttemptEvidence {
    /// The durable model-request lifecycle of this attempt.
    request: ExternalRequestLifecycle,
    /// The bounded summary of the attempt's foreground-tool external history.
    ///
    /// Independent of the per-call repair evidence: it keeps proving that
    /// external tool execution happened — and whether any external outcome
    /// remains unknown — after every detailed entry has been released.
    tools: ToolExternalSummary,
}

/// The external execution lifecycle of one tool call, as retained by the
/// per-call repair map.
///
/// This axis is deliberately separate from the canonical structure lifecycle:
/// a committed canonical `ToolResult` releases the entry entirely, while the
/// owning attempt's [`ToolExternalSummary`] independently remembers that the
/// external execution started.
#[derive(Debug, Clone, PartialEq)]
enum ToolExternalLifecycle {
    /// `ToolExecutionStarted` committed; no outcome fact followed. The
    /// external outcome is **unknown**.
    StartedOutcomeUnknown,
    /// A durable outcome fact exists (`ToolExecutionCompleted` or
    /// `ToolExecutionFailed`). Recovery uses this exact result, never an
    /// invented one.
    OutcomeKnown(Box<ToolExecutionResult>),
}

/// The bounded summary of one attempt's foreground-tool external execution
/// history.
///
/// The summary answers the classification questions — did external tool work
/// start, is any external outcome still unknown — **without** retaining every
/// historical `ToolExecutionResult`. It is monotonic during the fold:
/// `NeverStarted` can move into a started state, but a started state can
/// never move back to "never externally started".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum ToolExternalSummary {
    /// No `ToolExecutionStarted` and no durable tool outcome ever committed
    /// for this attempt. Only this state is consistent with Class B's "no
    /// `ToolExecutionStarted` ever" requirement.
    #[default]
    NeverStarted,
    /// External tool execution happened and every started call's outcome is
    /// durably known (`ToolExecutionCompleted`/`ToolExecutionFailed`).
    AllOutcomesKnown,
    /// External tool execution happened; at least one started call currently
    /// has no durable outcome (an outstanding repair-map entry).
    UnknownOutstanding,
    /// External tool execution happened; a started call whose outcome was
    /// unknown had its canonical `ToolResult` committed. The external outcome
    /// of that call can never become durably known — the recovery `Interrupted`
    /// shape is a canonical representation that the old outcome remains
    /// unknowable, never evidence that it became known — so the attempt keeps
    /// an unknown outcome until it terminalizes, even when a durable outcome
    /// of a *different* call would otherwise resolve every outstanding
    /// repair entry.
    UnknownIrreversible,
}

/// What durable evidence says about one tool execution of one attempt, kept
/// **only while that call may still need canonical repair**.
///
/// The evidence is keyed by its owning attempt **and** call id: the durable
/// authority does not guarantee `ToolCallId` uniqueness across the whole
/// conversation lifetime (providers mint call ids; only the active Surface
/// is uniqueness-checked), so events of historical attempts must never
/// alias the current unresolved call.
///
/// Absence from the repair map means "this call needs no further canonical
/// repair"; the owning attempt's [`ToolExternalSummary`] separately remembers
/// the historical external execution.
#[derive(Debug, Clone, PartialEq)]
struct ToolRepairEvidence {
    /// The executed tool.
    tool_id: ToolId,
    /// The external execution lifecycle, whose exact result (or honest
    /// unknown) is required to produce the missing canonical `ToolResult`.
    lifecycle: ToolExternalLifecycle,
}

/// What durable evidence says about one detached background execution whose
/// terminal publication is not committed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackgroundEvidence {
    /// The detached execution identity.
    pub execution_id: ToolExecutionId,
    /// The model-issued tool call it belongs to.
    pub tool_call_id: ToolCallId,
    /// The executed tool.
    pub tool_id: ToolId,
    /// The model-facing tool name frozen at dispatch time.
    pub tool_name: String,
}

/// What durable evidence says about one owned subagent child whose
/// terminal publication is not committed (Issue #60).
///
/// A v1 child is one-shot process-local work: the evidence exists so a
/// restart can settle the ownership honestly as interrupted, never to
/// reattach to or replay the old child process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubagentEvidence {
    /// The owned subagent identity.
    pub subagent_id: SubagentId,
    /// The child agent identity (the provenance its successful result
    /// would have carried).
    pub child_agent_id: crate::runtime::identity::AgentId,
    /// The child's own durable conversation identity.
    pub child_conversation_id: ConversationId,
    /// The model-issued tool call that delegated the work.
    pub tool_call_id: ToolCallId,
    /// The frozen child profile identity.
    pub profile: String,
}

/// The complete durable evidence of one conversation at process startup.
///
/// Every field comes from the durable authority alone. Nothing is derived from
/// current configuration, current plugin/Skill discovery, a Runtime Client
/// snapshot, or the current filesystem.
#[derive(Debug, Clone, PartialEq)]
pub struct RecoveryEvidence {
    conversation_id: ConversationId,
    /// The current Surface head's active canonical messages, in model-visible
    /// order.
    active: Vec<MessageBlock>,
    /// Accepted-but-not-yet-adopted inbound, exactly as #63 committed it.
    pending: Vec<PendingInboundItem>,
    /// Non-terminal attempts. The admission invariant permits at most one; a
    /// durable authority that holds two is a contract violation that
    /// reconciliation reports and fails closed on, rather than silently
    /// settling whichever attempt happens to sort first.
    unsettled_attempts: BTreeMap<AttemptId, AttemptEvidence>,
    /// Tool executions with durable external-start evidence, keyed by
    /// owning attempt and call id, retained **only while their canonical
    /// `ToolResult` may still need repair**.
    ///
    /// The composite key is the identity fix of the evidence model: the
    /// durable authority does not guarantee `ToolCallId` uniqueness across
    /// the whole conversation lifetime (only the active Surface rejects
    /// duplicates), so evidence of historical attempts can never alias the
    /// current unresolved call.
    ///
    /// A committed `ToolMessageCommitted` removes the entry; the owning
    /// attempt's [`AttemptEvidence`] tool summary independently remembers the
    /// historical external execution, so releasing this detail never regresses
    /// the attempt to "never externally started".
    tool_repairs: BTreeMap<(AttemptId, ToolCallId), ToolRepairEvidence>,
    /// The owning attempt of every **active** Assistant message, resolved
    /// from the `AssistantMessageCommitted` envelope.
    ///
    /// Bounded by the active Surface. This is what lets the tool repair
    /// attribute an active call to the exact attempt that issued it — a
    /// historical attempt's same-named call can never be mistaken for the
    /// current one.
    assistant_attempts: BTreeMap<MessageId, AttemptId>,
    /// The active message identities, for the bounded attribution above.
    active_ids: std::collections::BTreeSet<MessageId>,
    /// Background executions durably owned and not durably published.
    unsettled_background: Vec<BackgroundEvidence>,
    /// Subagent children durably owned and not durably published (Issue #60).
    unsettled_subagents: Vec<SubagentEvidence>,
    /// The highest conversation-scoped attempt ordinal that entered durable
    /// authority, terminal or not.
    highest_attempt_ordinal: Option<u64>,
    /// The highest background execution ordinal that entered durable
    /// authority, published or not.
    highest_background_ordinal: u64,
    /// The highest subagent ordinal that entered durable authority,
    /// published or not.
    highest_subagent_ordinal: u64,
    /// Whether the Event Journal contains any attempt fact at all.
    saw_any_attempt: bool,
}

impl RecoveryEvidence {
    /// **Phase 1.** Reads every durable fact recovery is allowed to consume.
    ///
    /// This phase commits nothing, invokes no provider, starts no tool or
    /// process, fabricates no observation, and executes no context
    /// contributor.
    ///
    /// # Errors
    ///
    /// Returns [`RecoveryError::Durable`] when the durable authority cannot be
    /// read coherently. Recovery then fails closed.
    pub fn reconstruct(store: &dyn ConversationStore) -> Result<Self, RecoveryError> {
        let conversation_id = store.conversation_id().clone();
        let head = store.load_head()?;
        let active = store.load_messages(&head.active_message_ids)?;
        let pending = store.load_pending()?;

        let mut evidence = Self {
            conversation_id,
            active,
            pending,
            unsettled_attempts: BTreeMap::new(),
            tool_repairs: BTreeMap::new(),
            unsettled_background: Vec::new(),
            unsettled_subagents: Vec::new(),
            assistant_attempts: BTreeMap::new(),
            active_ids: std::collections::BTreeSet::new(),
            highest_attempt_ordinal: None,
            highest_background_ordinal: 0,
            highest_subagent_ordinal: 0,
            saw_any_attempt: false,
        };
        evidence.active_ids = evidence
            .active
            .iter()
            .map(crate::conversation::message_id_of)
            .collect();

        // The bounded fold. Each page is decoded, folded, and dropped; only
        // the unresolved working set survives a page boundary.
        let mut cursor = None;
        let mut background: BTreeMap<ToolExecutionId, BackgroundEvidence> = BTreeMap::new();
        let mut subagents: BTreeMap<SubagentId, SubagentEvidence> = BTreeMap::new();
        loop {
            let page = store.read_events(cursor, RECOVERY_PAGE)?;
            if page.events.is_empty() {
                break;
            }
            for envelope in &page.events {
                evidence.fold(envelope, &mut background, &mut subagents);
            }
            cursor = page.next_sequence;
            if cursor.is_none() {
                break;
            }
        }
        evidence.unsettled_background = background.into_values().collect();
        evidence.unsettled_subagents = subagents.into_values().collect();
        Ok(evidence)
    }

    /// Folds one durable event into the unresolved working set.
    #[allow(clippy::too_many_lines)] // One event vocabulary, one fold, one place.
    fn fold(
        &mut self,
        envelope: &RuntimeEventEnvelope,
        background: &mut BTreeMap<ToolExecutionId, BackgroundEvidence>,
        subagents: &mut BTreeMap<SubagentId, SubagentEvidence>,
    ) {
        match &envelope.event {
            RuntimeEvent::AttemptStarted { attempt_id } => {
                self.note_attempt(attempt_id);
                self.unsettled_attempts
                    .entry(attempt_id.clone())
                    .or_insert(AttemptEvidence {
                        request: ExternalRequestLifecycle::NeverStarted,
                        tools: ToolExternalSummary::default(),
                    });
            }
            RuntimeEvent::AttemptCompleted { attempt_id, .. }
            | RuntimeEvent::AttemptCancelled { attempt_id, .. }
            | RuntimeEvent::AttemptTimedOut { attempt_id }
            | RuntimeEvent::AttemptLimitExceeded { attempt_id, .. }
            | RuntimeEvent::AttemptFailed { attempt_id, .. } => {
                self.note_attempt(attempt_id);
                // A durable terminal is absorbing: the attempt leaves the
                // unresolved working set and never returns to it. Its tool
                // repair evidence survives independently: an incomplete
                // canonical turn that outlived its owning attempt (Class D)
                // must stay repairable from its durable outcome, and entries
                // are released only by their own `ToolMessageCommitted`.
                self.unsettled_attempts.remove(attempt_id);
            }
            RuntimeEvent::ModelRequestStarted { request_id, .. } => {
                if let Some(attempt) = self.current_attempt_mut(envelope) {
                    // The newest start is the in-flight request. The
                    // transition is monotonic: a started request can never
                    // become "never started" again.
                    attempt.request = ExternalRequestLifecycle::StartedOutcomeUnknown {
                        request_id: request_id.clone(),
                    };
                }
            }
            RuntimeEvent::ModelRequestCompleted { .. }
            | RuntimeEvent::ModelRequestFailed { .. } => {
                if let Some(attempt) = self.current_attempt_mut(envelope) {
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
                // Attribute every **active** Assistant message to the attempt
                // that committed it. A message retired from the Surface is
                // not retained, so the map is bounded by the active working
                // set.
                if let Some(attempt) = envelope.attempt_id.clone()
                    && self.active_ids.contains(message_id)
                {
                    self.assistant_attempts.insert(message_id.clone(), attempt);
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
                    if let Some(evidence) = self.unsettled_attempts.get_mut(&attempt) {
                        evidence.tools = match evidence.tools {
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
                    self.tool_repairs.insert(
                        (attempt.clone(), tool_call_id.clone()),
                        ToolRepairEvidence {
                            tool_id: tool_id.clone(),
                            lifecycle: ToolExternalLifecycle::StartedOutcomeUnknown,
                        },
                    );
                }
            }
            RuntimeEvent::ToolExecutionCompleted {
                tool_call_id,
                tool_id,
                result,
            } => {
                if let Some(attempt) = envelope.attempt_id.clone() {
                    self.tool_repairs.insert(
                        (attempt.clone(), tool_call_id.clone()),
                        ToolRepairEvidence {
                            tool_id: tool_id.clone(),
                            lifecycle: ToolExternalLifecycle::OutcomeKnown(Box::new(
                                result.clone(),
                            )),
                        },
                    );
                    self.note_known_tool_outcome(&attempt);
                }
            }
            RuntimeEvent::ToolExecutionFailed {
                tool_call_id,
                tool_id,
                error,
            } => {
                if let Some(attempt) = envelope.attempt_id.clone() {
                    self.tool_repairs.insert(
                        (attempt.clone(), tool_call_id.clone()),
                        ToolRepairEvidence {
                            tool_id: tool_id.clone(),
                            lifecycle: ToolExternalLifecycle::OutcomeKnown(Box::new(
                                ToolExecutionResult {
                                    status: ToolExecutionStatus::Failed {
                                        error: error.clone(),
                                    },
                                    content: Vec::new(),
                                    duration_ms: 0,
                                    exit_code: None,
                                    artifacts: Vec::new(),
                                    truncation: None,
                                },
                            )),
                        },
                    );
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
                    // owning attempt resolves through the active assistant
                    // attribution — never through a bare call-id scan that
                    // could mark a historical leftover.
                    let mut owned_by = None;
                    for (assistant_id, attempt) in &self.assistant_attempts {
                        let expected = format!("{assistant_id}-recovered-tool-{tool_call_id}");
                        if message_id.as_str() == expected {
                            owned_by = Some(attempt.clone());
                            break;
                        }
                    }
                    owned_by
                };
                if let Some(attempt) = owning {
                    let key = (attempt.clone(), tool_call_id.clone());
                    // A call whose external outcome was still unknown when its
                    // canonical result committed has an outcome that can never
                    // become durably known: the recovery `Interrupted` shape
                    // is the canonical representation of "still unknowable",
                    // not evidence of a known outcome. The owning non-terminal
                    // attempt therefore keeps an unknown external outcome
                    // until it terminalizes.
                    let was_unknown = matches!(
                        self.tool_repairs.get(&key).map(|repair| &repair.lifecycle),
                        Some(ToolExternalLifecycle::StartedOutcomeUnknown)
                    );
                    // The canonical repair is settled: detailed per-call
                    // evidence is released whether the owner is terminal or
                    // not.
                    self.tool_repairs.remove(&key);
                    if was_unknown && let Some(evidence) = self.unsettled_attempts.get_mut(&attempt)
                    {
                        // The released call's external outcome can never
                        // become durably known.
                        evidence.tools = ToolExternalSummary::UnknownIrreversible;
                    }
                }
            }
            RuntimeEvent::BackgroundExecutionCommitted {
                execution_id,
                tool_call_id,
                tool_id,
                tool_name,
            } => {
                if let Some(ordinal) = execution_id.background_ordinal() {
                    self.highest_background_ordinal = self.highest_background_ordinal.max(ordinal);
                }
                background.insert(
                    execution_id.clone(),
                    BackgroundEvidence {
                        execution_id: execution_id.clone(),
                        tool_call_id: tool_call_id.clone(),
                        tool_id: tool_id.clone(),
                        tool_name: tool_name.clone(),
                    },
                );
            }
            RuntimeEvent::BackgroundTerminalPublished { execution_id, .. } => {
                if let Some(ordinal) = execution_id.background_ordinal() {
                    self.highest_background_ordinal = self.highest_background_ordinal.max(ordinal);
                }
                // The terminal publication is absorbing.
                background.remove(execution_id);
            }
            RuntimeEvent::SubagentOwnershipCommitted {
                subagent_id,
                child_agent_id,
                child_conversation_id,
                tool_call_id,
                profile,
            } => {
                if let Some(ordinal) = subagent_id.conversation_ordinal(&self.conversation_id) {
                    self.highest_subagent_ordinal = self.highest_subagent_ordinal.max(ordinal);
                }
                subagents.insert(
                    subagent_id.clone(),
                    SubagentEvidence {
                        subagent_id: subagent_id.clone(),
                        child_agent_id: child_agent_id.clone(),
                        child_conversation_id: child_conversation_id.clone(),
                        tool_call_id: tool_call_id.clone(),
                        profile: profile.clone(),
                    },
                );
            }
            RuntimeEvent::SubagentTerminalPublished { subagent_id, .. } => {
                if let Some(ordinal) = subagent_id.conversation_ordinal(&self.conversation_id) {
                    self.highest_subagent_ordinal = self.highest_subagent_ordinal.max(ordinal);
                }
                // The terminal publication is absorbing.
                subagents.remove(subagent_id);
            }
            _ => {
                if let Some(attempt_id) = envelope.attempt_id.as_ref() {
                    self.note_attempt(attempt_id);
                }
            }
        }
    }

    /// Records that this attempt identity entered durable authority.
    fn note_attempt(&mut self, attempt_id: &AttemptId) {
        self.saw_any_attempt = true;
        if let Some(ordinal) = attempt_id.conversation_ordinal(&self.conversation_id) {
            self.highest_attempt_ordinal = Some(
                self.highest_attempt_ordinal
                    .map_or(ordinal, |seen| seen.max(ordinal)),
            );
        }
    }

    fn current_attempt_mut(
        &mut self,
        envelope: &RuntimeEventEnvelope,
    ) -> Option<&mut AttemptEvidence> {
        let attempt_id = envelope.attempt_id.clone()?;
        self.note_attempt(&attempt_id);
        self.unsettled_attempts.get_mut(&attempt_id)
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
        let Some(evidence) = self.unsettled_attempts.get_mut(attempt_id) else {
            return;
        };
        if evidence.tools == ToolExternalSummary::UnknownIrreversible {
            return;
        }
        let outstanding_unknown = self.tool_repairs.iter().any(|((owning, _), repair)| {
            owning == attempt_id
                && matches!(
                    repair.lifecycle,
                    ToolExternalLifecycle::StartedOutcomeUnknown
                )
        });
        evidence.tools = if outstanding_unknown {
            ToolExternalSummary::UnknownOutstanding
        } else {
            ToolExternalSummary::AllOutcomesKnown
        };
    }

    /// The durable per-call repair evidence answering `call_id`, attributed
    /// to the exact owning attempt of the active Assistant message that
    /// issued it.
    ///
    /// The durable authority does not guarantee `ToolCallId` uniqueness
    /// across the conversation lifetime, so a bare call-id lookup could let
    /// a historical attempt's evidence alias the current unresolved call.
    /// The attribution comes from the `AssistantMessageCommitted` envelope
    /// (see [`RecoveryEvidence::assistant_attempts`]); a message without an
    /// attributed attempt (a bootstrapped turn) has no start evidence by
    /// construction and answers `None` — the honest "never started" case.
    ///
    /// An entry exists only while the call may still need canonical repair;
    /// a committed `ToolMessageCommitted` has released it.
    fn tool_repair_for(
        &self,
        call_id: &ToolCallId,
        owning_attempt: Option<&AttemptId>,
    ) -> Option<&ToolRepairEvidence> {
        let attempt = owning_attempt?;
        self.tool_repairs.get(&(attempt.clone(), call_id.clone()))
    }

    /// The active model-visible messages of the current durable Surface head.
    #[must_use]
    pub fn active_messages(&self) -> &[MessageBlock] {
        &self.active
    }

    /// The accepted-but-not-yet-adopted durable inbound.
    #[must_use]
    pub fn pending_inbound(&self) -> &[PendingInboundItem] {
        &self.pending
    }

    /// The next free conversation-scoped attempt ordinal.
    ///
    /// Never an ordinal that already entered durable authority.
    #[must_use]
    pub fn next_attempt_ordinal(&self) -> u64 {
        self.highest_attempt_ordinal
            .map_or(0, |seen| seen.saturating_add(1))
    }

    /// The highest background execution ordinal in durable authority.
    #[must_use]
    pub fn highest_background_ordinal(&self) -> u64 {
        self.highest_background_ordinal
    }
}

// ---------------------------------------------------------------------------
// Phase 2 — classify
// ---------------------------------------------------------------------------

/// The deterministic classification of the crash-time attempt plane.
///
/// The classification is a pure function of the durable evidence. It never
/// depends on wall-clock timing, current provider availability, current
/// plugin/config state, whether a Runtime Client is attached, or any random
/// retry decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttemptRecoveryClass {
    /// **Class A.** No durable attempt evidence exists at all. Accepted
    /// Pending Inbound (if any) remains authoritative and is ordinary
    /// admissible work.
    NotStarted,
    /// **Class B.** An attempt was admitted and its inbound was already
    /// canonicalized, but no external side effect ever crossed a start
    /// commit: no `ModelRequestStarted`, no `ToolExecutionStarted`. The
    /// process-local execution state is gone, so the old attempt receives an
    /// explicit interrupted recovery terminal — and because nothing external
    /// happened, the already-canonical turn may safely continue through a
    /// **new** attempt without re-adopting or duplicating the `UserMessage`.
    AdmittedWithoutExternalStart {
        /// The interrupted attempt.
        attempt_id: AttemptId,
    },
    /// **Class C.** A model request and/or a tool execution crossed its
    /// durable start commit and no outcome is durably known.
    ///
    /// This is the critical class: the provider may have received and executed
    /// the request; the tool may have completed its external effect. The
    /// ambiguity is preserved as a first-class fact. There is **no** automatic
    /// resend and **no** automatic re-execution. Unknown dominates mixed
    /// states: a known result elsewhere never hides one started side effect
    /// whose outcome is unknown.
    ///
    /// `tool_calls` names the calls whose unknown outcome is still repairable;
    /// a call whose canonical result already committed (the recovery
    /// `Interrupted` shape) is no longer named — its external outcome remains
    /// unknowable and still blocks continuation, but the call identity is
    /// released with the repair evidence.
    IndeterminateExternalOutcome {
        /// The interrupted attempt.
        attempt_id: AttemptId,
        /// The started request whose provider outcome is unknown, if any.
        model_request: Option<RequestId>,
        /// The started tool calls whose external outcome is unknown and
        /// whose canonical `ToolResult` is still outstanding.
        tool_calls: Vec<ToolCallId>,
    },
    /// **Class D.** Every durable attempt already carries its one terminal
    /// fact. The state is absorbing: recovery adds no second terminal, and
    /// repeated restarts change nothing.
    AlreadyTerminal,
    /// **Class E.** External work crossed a durable start commit and its
    /// outcome is **durably known**, but the canonical/attempt settlement
    /// did not commit before the crash.
    ///
    /// The known outcome is preserved (the exact durable tool result is
    /// repaired into the canonical Surface; the provider outcome stays a
    /// durable fact), the dead attempt is terminalized honestly, and —
    /// critically — this is **never** described as "no external start": no
    /// automatic resend and no automatic re-execution. A known request
    /// completion also never fabricates the Assistant response body, which
    /// never became canonical.
    ///
    /// `tool_calls` names the calls whose known outcome is still repairable;
    /// a call whose canonical result already committed is no longer named.
    ExternalOutcomeKnown {
        /// The interrupted attempt.
        attempt_id: AttemptId,
        /// The started model request whose outcome is durably known, if
        /// any. Carries the outcome so the recovery terminal can state the
        /// strongest honest fact.
        model_request: Option<KnownModelOutcome>,
        /// The started tool calls whose outcome is durably known and whose
        /// canonical `ToolResult` is still outstanding.
        tool_calls: Vec<ToolCallId>,
    },
}

/// The durably known outcome of the one started model request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnownModelOutcome {
    /// The request, when its start fact committed durably.
    pub request_id: Option<RequestId>,
    /// What the provider outcome was.
    pub outcome: RequestOutcome,
}

/// The recovery classification of one detached background execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackgroundRecoveryClass {
    /// The durably owned execution.
    pub evidence: BackgroundEvidence,
}

/// The recovery classification of one owned subagent child (Issue #60).
///
/// A v1 child is one-shot and process-local: the only honest nonterminal
/// recovery outcome is interruption. There is no reattach and no replay
/// classification by construction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubagentRecoveryClass {
    /// The durably owned child.
    pub evidence: SubagentEvidence,
}

/// What the recovered runtime is permitted to continue.
///
/// A permission, never an obligation to replay: the conversation runtime
/// consumes it at activation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResumeDisposition {
    /// Only ordinary Pending Inbound admission. Nothing else is outstanding.
    PendingInboundOnly,
    /// The adopted-but-unanswered canonical turn may continue through one new
    /// attempt (Class B). The `UserMessage` is already canonical and is never
    /// re-adopted.
    ContinueAdoptedTurn,
    /// Continuation is blocked because an external outcome is indeterminate
    /// (Class C). Pending Inbound remains admissible — that is new
    /// user/producer-driven work, not a replay of the ambiguous request — but
    /// recovery itself starts nothing.
    BlockedIndeterminate,
}

/// The deterministic plan produced by classification.
#[derive(Debug, Clone, PartialEq)]
pub struct RecoveryPlan {
    conversation_id: ConversationId,
    attempt: AttemptRecoveryClass,
    background: Vec<BackgroundRecoveryClass>,
    subagents: Vec<SubagentRecoveryClass>,
    /// The missing canonical `ToolResult` siblings, grouped by their owning
    /// Assistant message, in canonical model-call order.
    tool_repairs: Vec<ToolTurnRepair>,
    resume: ResumeDisposition,
    next_attempt_ordinal: u64,
    highest_background_ordinal: u64,
    highest_subagent_ordinal: u64,
    pending_inbound: usize,
    /// Every durably non-terminal attempt the fold observed.
    ///
    /// The admission invariant permits at most one, so this is normally the
    /// same single attempt the classification names. It is retained so that a
    /// durable authority which disagrees with that invariant is *reported*
    /// rather than silently truncated to whichever attempt sorted first.
    unsettled_attempts: Vec<AttemptId>,
    /// The foreground-tool external summary of the classified attempt, kept
    /// only so the recovery-terminal diagnostic stays truthful about tool
    /// calls whose detailed repair evidence has already been released.
    ///
    /// The classification itself is fully determined by the enum; this field
    /// is diagnostic context, never class evidence.
    tool_summary: Option<ToolExternalSummary>,
}

/// One structurally incomplete canonical tool turn and its repair batch.
#[derive(Debug, Clone, PartialEq)]
struct ToolTurnRepair {
    /// The Assistant message that issued the calls.
    assistant_message_id: MessageId,
    /// The missing siblings, in the Assistant message's own call order.
    missing: Vec<MissingToolResult>,
}

/// One missing canonical `ToolResult` and the durable evidence behind it.
#[derive(Debug, Clone, PartialEq)]
struct MissingToolResult {
    call_id: ToolCallId,
    tool_id: ToolId,
    result: ToolExecutionResult,
}

impl RecoveryPlan {
    /// **Phase 2.** Classifies the durable evidence deterministically.
    ///
    /// Pure: the same evidence always yields the same plan.
    #[must_use]
    pub fn classify(evidence: &RecoveryEvidence) -> Self {
        let tool_repairs = Self::plan_tool_repairs(evidence);
        let attempt = Self::classify_attempt(evidence);
        let resume = match &attempt {
            AttemptRecoveryClass::AdmittedWithoutExternalStart { .. }
                if awaits_model_turn(&evidence.active) =>
            {
                ResumeDisposition::ContinueAdoptedTurn
            }
            AttemptRecoveryClass::IndeterminateExternalOutcome { .. } => {
                ResumeDisposition::BlockedIndeterminate
            }
            _ => ResumeDisposition::PendingInboundOnly,
        };
        Self {
            conversation_id: evidence.conversation_id.clone(),
            attempt,
            background: evidence
                .unsettled_background
                .iter()
                .map(|evidence| BackgroundRecoveryClass {
                    evidence: evidence.clone(),
                })
                .collect(),
            subagents: evidence
                .unsettled_subagents
                .iter()
                .map(|evidence| SubagentRecoveryClass {
                    evidence: evidence.clone(),
                })
                .collect(),
            tool_repairs,
            resume,
            next_attempt_ordinal: evidence.next_attempt_ordinal(),
            highest_background_ordinal: evidence.highest_background_ordinal,
            highest_subagent_ordinal: evidence.highest_subagent_ordinal,
            pending_inbound: evidence.pending.len(),
            unsettled_attempts: evidence.unsettled_attempts.keys().cloned().collect(),
            tool_summary: evidence.unsettled_attempts.values().next().map(|a| a.tools),
        }
    }

    fn classify_attempt(evidence: &RecoveryEvidence) -> AttemptRecoveryClass {
        let Some((attempt_id, attempt)) = evidence.unsettled_attempts.iter().next() else {
            return if evidence.saw_any_attempt {
                AttemptRecoveryClass::AlreadyTerminal
            } else {
                AttemptRecoveryClass::NotStarted
            };
        };
        // The class decision comes from the attempt's own external-history
        // summary — the bounded request lifecycle plus the bounded
        // foreground-tool summary. It never needs detailed historical
        // settled `ToolExecutionResult`s: a fully canonicalized call whose
        // repair evidence was released still contributes to the bounded
        // external summary here.
        let summary = &attempt.tools;
        // The still-namable calls of **this** attempt from the outstanding
        // per-call repair evidence. A historical attempt's unresolved tool
        // (a Class-D leftover) must never make the crash-time attempt
        // indeterminate: the ambiguity of a settled attempt belongs to that
        // attempt's own terminal, not to the current one. And a released
        // entry (canonical repair already committed) can no longer be named
        // — the lists are best-effort diagnostics, never the class evidence.
        let mut indeterminate_tools = Vec::new();
        let mut known_tools = Vec::new();
        for ((owning, call), repair) in &evidence.tool_repairs {
            if owning != attempt_id {
                continue;
            }
            match &repair.lifecycle {
                ToolExternalLifecycle::StartedOutcomeUnknown => {
                    indeterminate_tools.push(call.clone());
                }
                ToolExternalLifecycle::OutcomeKnown(_) => {
                    known_tools.push(call.clone());
                }
            }
        }
        match &attempt.request {
            // The in-flight request's outcome is unknown: indeterminate,
            // never resendable.
            ExternalRequestLifecycle::StartedOutcomeUnknown { request_id } => {
                AttemptRecoveryClass::IndeterminateExternalOutcome {
                    attempt_id: attempt_id.clone(),
                    model_request: Some(request_id.clone()),
                    tool_calls: indeterminate_tools,
                }
            }
            // A tool execution with an unknown outcome is indeterminate,
            // whatever the request plane says: no request start ever, or a
            // durably known request outcome — the started tool may have
            // completed its external effect, so no resend and no
            // re-execution. Unknown dominates mixed states: a known result
            // elsewhere never hides one started side effect whose outcome is
            // unknown. The `model_request` field names only a request whose
            // outcome is unknown, which neither case is.
            ExternalRequestLifecycle::NeverStarted
            | ExternalRequestLifecycle::StartedOutcomeKnown { .. }
                if matches!(
                    summary,
                    ToolExternalSummary::UnknownOutstanding
                        | ToolExternalSummary::UnknownIrreversible
                ) =>
            {
                AttemptRecoveryClass::IndeterminateExternalOutcome {
                    attempt_id: attempt_id.clone(),
                    model_request: None,
                    tool_calls: indeterminate_tools,
                }
            }
            // **Zero** durable external-start evidence: only this state is
            // eligible for the Class-B continuation. The repair-map guard
            // is defensive — a summary and its repair entries are written in
            // the same fold transition, so for an orderly journal it is
            // implied by the summary bits; it makes "never externally
            // started" airtight against event-order anomalies.
            ExternalRequestLifecycle::NeverStarted
                if *summary == ToolExternalSummary::NeverStarted
                    && !evidence
                        .tool_repairs
                        .keys()
                        .any(|(owning, _)| owning == attempt_id) =>
            {
                AttemptRecoveryClass::AdmittedWithoutExternalStart {
                    attempt_id: attempt_id.clone(),
                }
            }
            // External work crossed a start commit and every durable
            // outcome is known, but the canonical/attempt settlement did
            // not commit before the crash. Never "no external start",
            // never replayed.
            _ => {
                let model_request = match &attempt.request {
                    ExternalRequestLifecycle::StartedOutcomeKnown {
                        request_id,
                        outcome,
                    } => Some(KnownModelOutcome {
                        request_id: request_id.clone(),
                        outcome: *outcome,
                    }),
                    _ => None,
                };
                AttemptRecoveryClass::ExternalOutcomeKnown {
                    attempt_id: attempt_id.clone(),
                    model_request,
                    tool_calls: known_tools,
                }
            }
        }
    }

    /// Plans the canonical repair of every structurally incomplete tool turn
    /// on the current Surface, using **only** durable evidence.
    ///
    /// The repair is planned from the Surface structure rather than from the
    /// attempt plane, because an incomplete tool turn can outlive its attempt:
    /// an already-terminal attempt (Class D) that crashed between its terminal
    /// and its result batch must be repaired exactly the same way.
    fn plan_tool_repairs(evidence: &RecoveryEvidence) -> Vec<ToolTurnRepair> {
        let mut answered: std::collections::BTreeSet<ToolCallId> =
            std::collections::BTreeSet::new();
        for message in &evidence.active {
            if let MessageBlock::Tool(tool) = message {
                answered.insert(tool.tool_call_id.clone());
            }
        }
        let mut repairs = Vec::new();
        for message in &evidence.active {
            let MessageBlock::Assistant(assistant) = message else {
                continue;
            };
            let mut missing = Vec::new();
            for block in &assistant.content {
                let AssistantContentBlock::ToolCall(call) = block else {
                    continue;
                };
                if answered.contains(&call.id) {
                    continue;
                }
                // The exact owning attempt of this active call, from the
                // `AssistantMessageCommitted` envelope. A call of a message
                // with no attributed attempt (a bootstrapped turn) has no
                // start evidence by construction.
                let owning = evidence
                    .assistant_attempts
                    .get(&crate::conversation::message_id_of(message));
                let repair = evidence.tool_repair_for(&call.id, owning);
                missing.push(match repair {
                    // The external effect started and no outcome is durably
                    // known: the strongest honest native status.
                    Some(ToolRepairEvidence {
                        lifecycle: ToolExternalLifecycle::StartedOutcomeUnknown,
                        tool_id,
                    }) => MissingToolResult {
                        call_id: call.id.clone(),
                        tool_id: tool_id.clone(),
                        result: interrupted_result(),
                    },
                    // The outcome *is* durably known; the canonical message
                    // simply never committed. The durable result is used
                    // verbatim — no invented body, no completion race.
                    Some(ToolRepairEvidence {
                        lifecycle: ToolExternalLifecycle::OutcomeKnown(result),
                        tool_id,
                    }) => MissingToolResult {
                        call_id: call.id.clone(),
                        tool_id: tool_id.clone(),
                        result: (**result).clone(),
                    },
                    // Durable evidence says this sibling never started, so
                    // nothing external happened and nothing is unknown: it was
                    // abandoned with its owning attempt.
                    None => MissingToolResult {
                        call_id: call.id.clone(),
                        tool_id: call.tool_id.clone(),
                        result: ToolExecutionResult {
                            status: ToolExecutionStatus::Cancelled {
                                reason: CancellationReason::ParentCancelled,
                            },
                            content: Vec::new(),
                            duration_ms: 0,
                            exit_code: None,
                            artifacts: Vec::new(),
                            truncation: None,
                        },
                    },
                });
            }
            if !missing.is_empty() {
                repairs.push(ToolTurnRepair {
                    assistant_message_id: crate::conversation::message_id_of(message),
                    missing,
                });
            }
        }
        repairs
    }

    /// The classified attempt plane.
    #[must_use]
    pub fn attempt_class(&self) -> &AttemptRecoveryClass {
        &self.attempt
    }

    /// The classified background plane.
    #[must_use]
    pub fn background_classes(&self) -> &[BackgroundRecoveryClass] {
        &self.background
    }

    /// The classified subagent plane (Issue #60).
    #[must_use]
    pub fn subagent_classes(&self) -> &[SubagentRecoveryClass] {
        &self.subagents
    }

    /// What the recovered runtime is permitted to continue.
    #[must_use]
    pub fn resume(&self) -> ResumeDisposition {
        self.resume
    }

    /// **Phase 3.** Commits every required recovery fact, each as one atomic
    /// durable transition with an explicit linearization point.
    ///
    /// The order is fixed and load-bearing:
    ///
    /// ```text
    /// 1. canonical tool-turn repair   -> the Surface can form a valid model request again
    /// 2. attempt recovery terminal    -> the interrupted attempt settles exactly once
    /// 3. background terminal publication -> the model-visible notification, exactly once
    /// ```
    ///
    /// Repairing the structure before terminalizing the attempt is what keeps
    /// the repair legal: an attempt lifecycle that is already terminal accepts
    /// no further attempt-scoped fact. Recovery-generated canonical facts
    /// therefore carry **no** attempt identity — they are facts of the startup
    /// recovery phase, not of the attempt that died.
    ///
    /// Every transition is prepared in memory, committed atomically, and only
    /// then reflected in the returned report. A failure aborts recovery: no
    /// runtime is produced and no success is published.
    ///
    /// # Errors
    ///
    /// Returns [`RecoveryError::Durable`] when a reconciliation transaction
    /// fails, and [`RecoveryError::Unrecoverable`] when the durable authority
    /// contradicts the one-active-attempt invariant.
    pub fn reconcile(
        self,
        store: &dyn ConversationStore,
        clock: &dyn RuntimeClock,
    ) -> Result<RecoveryReport, RecoveryError> {
        // A conversation admits at most one attempt at a time, so at most one
        // attempt can be durably non-terminal. Two would mean the durable
        // authority contradicts the admission invariant, and settling only the
        // first would silently hide the other. Recovery reports it and fails
        // closed instead of guessing which attempt is real.
        if self.unsettled_attempts.len() > 1 {
            return Err(RecoveryError::Unrecoverable(format!(
                "the durable authority holds {} concurrently non-terminal attempts ({}), \
                 but a conversation admits at most one attempt at a time",
                self.unsettled_attempts.len(),
                self.unsettled_attempts
                    .iter()
                    .map(AttemptId::as_str)
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        }
        let mut committed = RecoveryReconciliation::default();
        self.repair_tool_turns(store, clock, &mut committed)?;
        self.settle_interrupted_attempt(store, clock, &mut committed)?;
        self.publish_background_terminals(store, clock, &mut committed)?;
        self.publish_subagent_terminals(store, clock, &mut committed)?;
        Ok(RecoveryReport {
            attempt: self.attempt,
            background: self.background,
            subagents: self.subagents,
            resume: self.resume,
            reconciliation: committed,
            next_attempt_ordinal: self.next_attempt_ordinal,
            highest_background_ordinal: self.highest_background_ordinal,
            highest_subagent_ordinal: self.highest_subagent_ordinal,
            pending_inbound: self.pending_inbound,
        })
    }

    /// **Reconciliation 1.** Completes every structurally incomplete canonical
    /// tool turn, one atomic sibling batch per owning Assistant message.
    ///
    /// Before the commit the turn cannot form a valid later model request;
    /// after it, every issued call owns exactly one committed `ToolResult`. A
    /// durable prefix of a sibling batch is never observable.
    fn repair_tool_turns(
        &self,
        store: &dyn ConversationStore,
        clock: &dyn RuntimeClock,
        committed: &mut RecoveryReconciliation,
    ) -> Result<(), RecoveryError> {
        for repair in &self.tool_repairs {
            let mut blocks = Vec::with_capacity(repair.missing.len());
            let mut events = Vec::with_capacity(repair.missing.len());
            for missing in &repair.missing {
                let message_id = MessageId::new(format!(
                    "{}-recovered-tool-{}",
                    repair.assistant_message_id, missing.call_id
                ));
                blocks.push(MessageBlock::Tool(ToolMessageBlock {
                    id: message_id.clone(),
                    tool_call_id: missing.call_id.clone(),
                    tool_id: missing.tool_id.clone(),
                    result: missing.result.clone(),
                }));
                events.push(self.recovery_envelope(
                    EventId::new(format!("recovery-tool-committed:{message_id}")),
                    RuntimeEvent::ToolMessageCommitted {
                        message_id,
                        tool_call_id: missing.call_id.clone(),
                    },
                    clock.now(),
                ));
            }
            store.append_canonical_batch_with_events(&blocks, &events)?;
            committed
                .repaired_tool_results
                .extend(repair.missing.iter().map(|missing| missing.call_id.clone()));
        }
        Ok(())
    }

    /// **Reconciliation 2.** Settles the one interrupted attempt, when the
    /// classification found one.
    ///
    /// The diagnostic states exactly what rustX knows and what stays unknown;
    /// it never claims the external work failed.
    #[allow(clippy::too_many_lines)] // One per-class diagnostic match, one place.
    fn settle_interrupted_attempt(
        &self,
        store: &dyn ConversationStore,
        clock: &dyn RuntimeClock,
        committed: &mut RecoveryReconciliation,
    ) -> Result<(), RecoveryError> {
        let (attempt_id, diagnostic) = match &self.attempt {
            AttemptRecoveryClass::AdmittedWithoutExternalStart { attempt_id } => (
                attempt_id,
                format!(
                    "the runtime restarted while attempt {attempt_id} was durably non-terminal; \
                     no model request and no tool execution had crossed a durable start commit, \
                     so no external side effect is outstanding"
                ),
            ),
            AttemptRecoveryClass::IndeterminateExternalOutcome {
                attempt_id,
                model_request,
                tool_calls,
            } => {
                let request = model_request.as_ref().map_or_else(
                    || "no model request outcome was unknown".to_owned(),
                    |request| {
                        format!(
                            "model request {request} started and its provider outcome is unknown"
                        )
                    },
                );
                let tools = if tool_calls.is_empty() {
                    // No still-repairable call can be named. Distinguish the
                    // honest readings: an unknown that is purely the model
                    // request (no tool was ever indeterminate), versus a
                    // started tool whose unknown outcome was already
                    // canonically committed as `Interrupted` — which keeps
                    // the external outcome unknowable.
                    if self.tool_summary.is_some_and(|summary| {
                        matches!(
                            summary,
                            ToolExternalSummary::UnknownOutstanding
                                | ToolExternalSummary::UnknownIrreversible
                        )
                    }) {
                        "a started tool execution's external outcome remains unknown".to_owned()
                    } else {
                        "no tool execution outcome is indeterminate".to_owned()
                    }
                } else {
                    format!(
                        "the external outcome of tool call(s) {} is unknown",
                        tool_calls
                            .iter()
                            .map(ToolCallId::as_str)
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                };
                (
                    attempt_id,
                    format!(
                        "the runtime restarted while attempt {attempt_id} was durably \
                         non-terminal; {request}; {tools}. Nothing was resent and nothing was \
                         re-executed"
                    ),
                )
            }
            AttemptRecoveryClass::ExternalOutcomeKnown {
                attempt_id,
                model_request,
                tool_calls,
            } => {
                let request = model_request.as_ref().map_or_else(
                    || "no model request outcome was pending".to_owned(),
                    |known| match known.outcome {
                        RequestOutcome::Completed => format!(
                            "model request {} completed durably; its Assistant message \
                             never became canonical, so no response body is fabricated",
                            known
                                .request_id
                                .as_ref()
                                .map_or_else(|| "(identity not durable)", RequestId::as_str)
                        ),
                        RequestOutcome::Failed => format!(
                            "model request {} failed durably; the historical failure is \
                             preserved and was not retried",
                            known
                                .request_id
                                .as_ref()
                                .map_or_else(|| "(identity not durable)", RequestId::as_str)
                        ),
                    },
                );
                let tools = if tool_calls.is_empty() {
                    // No still-repairable call can be named. If the attempt's
                    // summary proves tool execution happened, its outcomes
                    // were durably known and the repairs already committed;
                    // otherwise no tool outcome was pending at all.
                    if self
                        .tool_summary
                        .is_some_and(|summary| summary != ToolExternalSummary::NeverStarted)
                    {
                        "the durably known outcome of a started tool execution is preserved"
                            .to_owned()
                    } else {
                        "no tool execution outcome was pending".to_owned()
                    }
                } else {
                    format!(
                        "the durably known outcome of tool call(s) {} is preserved",
                        tool_calls
                            .iter()
                            .map(ToolCallId::as_str)
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                };
                (
                    attempt_id,
                    format!(
                        "the runtime restarted while attempt {attempt_id} was durably \
                         non-terminal; {request}; {tools}. Nothing was resent and nothing \
                         was re-executed"
                    ),
                )
            }
            AttemptRecoveryClass::NotStarted | AttemptRecoveryClass::AlreadyTerminal => {
                return Ok(());
            }
        };
        self.terminalize(store, attempt_id, clock.now(), &diagnostic)?;
        committed.attempt_terminal = Some(attempt_id.clone());
        Ok(())
    }

    /// **Reconciliation 3.** Publishes the terminal notification of every
    /// durably owned background execution that never settled.
    ///
    /// The terminal Pending Inbound row and the `BackgroundTerminalPublished`
    /// fact commit in one transaction, exactly as the live settlement path
    /// does. The stable producer correlation and the durable
    /// `background:{execution_id}` terminal lifecycle together make this
    /// exactly-once across any number of restarts.
    fn publish_background_terminals(
        &self,
        store: &dyn ConversationStore,
        clock: &dyn RuntimeClock,
        committed: &mut RecoveryReconciliation,
    ) -> Result<(), RecoveryError> {
        for class in &self.background {
            let (draft, event) = crate::tools::background::recovery_terminal_publication(
                &self.conversation_id,
                &class.evidence.execution_id,
                &class.evidence.tool_name,
                clock.now(),
            );
            store.accept_inbound_with_event(draft, event)?;
            committed
                .background_terminals
                .push(class.evidence.execution_id.clone());
        }
        Ok(())
    }

    /// **Reconciliation 4.** Publishes the terminal notice of every durably
    /// owned subagent child that never settled (Issue #60).
    ///
    /// A v1 child does not survive its owning process: the notice states the
    /// honest interrupted outcome, and the durable `subagent:{subagent_id}`
    /// lifecycle plus the stable producer correlation make the publication
    /// exactly-once across any number of restarts. Nothing is reattached,
    /// relaunched, or replayed.
    fn publish_subagent_terminals(
        &self,
        store: &dyn ConversationStore,
        clock: &dyn RuntimeClock,
        committed: &mut RecoveryReconciliation,
    ) -> Result<(), RecoveryError> {
        for class in &self.subagents {
            let (draft, event) = crate::runtime::subagent::recovery_terminal_publication(
                &self.conversation_id,
                &class.evidence.subagent_id,
                &class.evidence.child_agent_id,
                &class.evidence.profile,
                clock.now(),
            );
            store.accept_inbound_with_event(draft, event)?;
            committed
                .subagent_terminals
                .push(class.evidence.subagent_id.clone());
        }
        Ok(())
    }

    fn terminalize(
        &self,
        store: &dyn ConversationStore,
        attempt_id: &AttemptId,
        timestamp: DateTime<Utc>,
        diagnostic: &str,
    ) -> Result<(), RecoveryError> {
        // The one terminal fact of the interrupted attempt. Its envelope
        // carries the attempt identity so the durable
        // `attempt:{attempt_id}` lifecycle closes; a second reconciliation
        // after a further restart is refused by that same lifecycle, and
        // classification never reaches this branch again because the attempt
        // is no longer unresolved.
        let envelope = RuntimeEventEnvelope {
            schema_version: crate::events::types::EVENT_SCHEMA_VERSION,
            event_id: EventId::new(format!("recovery-attempt-terminal:{attempt_id}")),
            sequence: 0,
            conversation_id: self.conversation_id.clone(),
            attempt_id: Some(attempt_id.clone()),
            turn_id: None,
            timestamp,
            event: RuntimeEvent::AttemptFailed {
                attempt_id: attempt_id.clone(),
                error: AttemptFailure::Runtime {
                    error: RuntimeError::RestartInterrupted {
                        message: diagnostic.to_owned(),
                    },
                },
            },
        };
        store.append_event(envelope)?;
        Ok(())
    }

    /// The envelope of one recovery-generated fact.
    ///
    /// Recovery facts carry no attempt or turn identity: they are new facts
    /// committed by the startup recovery phase, never retroactive claims about
    /// what the dead attempt did.
    fn recovery_envelope(
        &self,
        event_id: EventId,
        event: RuntimeEvent,
        timestamp: DateTime<Utc>,
    ) -> RuntimeEventEnvelope {
        RuntimeEventEnvelope {
            schema_version: crate::events::types::EVENT_SCHEMA_VERSION,
            event_id,
            sequence: 0,
            conversation_id: self.conversation_id.clone(),
            attempt_id: None,
            turn_id: None,
            timestamp,
            event,
        }
    }
}

// ---------------------------------------------------------------------------
// Phase 3 result / phase 4 permission
// ---------------------------------------------------------------------------

/// Exactly which new durable facts this recovery committed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RecoveryReconciliation {
    /// Tool calls whose missing canonical `ToolResult` was committed.
    pub repaired_tool_results: Vec<ToolCallId>,
    /// The attempt that received an interrupted recovery terminal.
    pub attempt_terminal: Option<AttemptId>,
    /// Background executions whose terminal notification was published.
    pub background_terminals: Vec<ToolExecutionId>,
    /// Subagent children whose interrupted terminal notice was published
    /// (Issue #60).
    pub subagent_terminals: Vec<SubagentId>,
}

impl RecoveryReconciliation {
    /// Whether this recovery committed no new durable fact at all.
    ///
    /// A second restart after a successful recovery is exactly this: durable
    /// state stops changing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.repaired_tool_results.is_empty()
            && self.attempt_terminal.is_none()
            && self.background_terminals.is_empty()
            && self.subagent_terminals.is_empty()
    }
}

/// The observable result of one startup recovery.
#[derive(Debug, Clone, PartialEq)]
pub struct RecoveryReport {
    attempt: AttemptRecoveryClass,
    background: Vec<BackgroundRecoveryClass>,
    subagents: Vec<SubagentRecoveryClass>,
    resume: ResumeDisposition,
    reconciliation: RecoveryReconciliation,
    next_attempt_ordinal: u64,
    highest_background_ordinal: u64,
    highest_subagent_ordinal: u64,
    pending_inbound: usize,
}

impl RecoveryReport {
    /// The deterministic attempt-plane classification.
    #[must_use]
    pub fn attempt_class(&self) -> &AttemptRecoveryClass {
        &self.attempt
    }

    /// The deterministic background-plane classification.
    #[must_use]
    pub fn background_classes(&self) -> &[BackgroundRecoveryClass] {
        &self.background
    }

    /// The deterministic subagent-plane classification (Issue #60).
    #[must_use]
    pub fn subagent_classes(&self) -> &[SubagentRecoveryClass] {
        &self.subagents
    }

    /// What the recovered runtime is permitted to continue.
    #[must_use]
    pub fn resume(&self) -> ResumeDisposition {
        self.resume
    }

    /// The new durable facts this recovery committed.
    #[must_use]
    pub fn reconciliation(&self) -> &RecoveryReconciliation {
        &self.reconciliation
    }

    /// The next free conversation-scoped attempt ordinal.
    #[must_use]
    pub fn next_attempt_ordinal(&self) -> u64 {
        self.next_attempt_ordinal
    }

    /// The highest background execution ordinal already in durable authority.
    #[must_use]
    pub fn highest_background_ordinal(&self) -> u64 {
        self.highest_background_ordinal
    }

    /// The highest subagent ordinal already in durable authority (Issue #60).
    #[must_use]
    pub fn highest_subagent_ordinal(&self) -> u64 {
        self.highest_subagent_ordinal
    }

    /// How many accepted-but-not-yet-adopted inbound items were recovered.
    #[must_use]
    pub fn pending_inbound(&self) -> usize {
        self.pending_inbound
    }
}

// ---------------------------------------------------------------------------
// The composed startup phase
// ---------------------------------------------------------------------------

/// Runs the complete startup recovery of one durable conversation.
///
/// This is the composition of the four phases, in order, and it is the only
/// entry point the conversation runtime uses. The phases remain separate
/// functions so each can be exercised — and reviewed — on its own.
///
/// The post-condition is checked, not assumed: after reconciliation the
/// current Surface must satisfy [`recovery_safety`]. If it does not, recovery
/// could not honestly repair the conversation and fails closed rather than
/// producing a runtime that would later trip the live admission guard.
///
/// # Errors
///
/// Returns [`RecoveryError::Durable`] on a durable read/commit failure and
/// [`RecoveryError::Unrecoverable`] when the reconciled state is still not at
/// a safe boundary.
pub fn recover(
    store: &dyn ConversationStore,
    clock: &dyn RuntimeClock,
) -> Result<RecoveryReport, RecoveryError> {
    let evidence = RecoveryEvidence::reconstruct(store)?;
    let plan = RecoveryPlan::classify(&evidence);
    let report = plan.reconcile(store, clock)?;
    // The post-condition of the whole pipeline, re-read from the durable
    // authority rather than from the pre-reconciliation working set.
    let head = store.load_head()?;
    let active = store.load_messages(&head.active_message_ids)?;
    if let Err(RecoverySafetyError::IncompleteToolTurn { tool_call_id }) = recovery_safety(&active)
    {
        return Err(RecoveryError::Unrecoverable(format!(
            "the reconciled Surface still ends inside an incomplete tool turn: tool call \
             {tool_call_id} has no committed ToolResult"
        )));
    }
    Ok(report)
}

/// Whether the current Surface ends in an adopted ordinary inbound turn that
/// is still awaiting its model answer.
///
/// Used only to decide whether a Class B continuation has anything to
/// continue. It inspects committed canonical structure — never a producer
/// state, a client cache, or a timestamp heuristic — and it deliberately
/// requires an ordinary [`InboundKind::Message`]: a runtime compaction summary
/// is user-role *history*, not unanswered work, and must never be mistaken for
/// a turn that needs an answer.
///
/// The continuation runs as an explicit `InitialTurnTrigger::Continuation`,
/// never as a reconstructed fresh-inbound turn. A `FreshInboundTurn` is
/// process-local **execution** state describing a batch this runtime adopted;
/// it was never durable, so recovery would have to fabricate it. Recovery
/// fabricates nothing: the recovered attempt continues committed canonical
/// history, which is exactly what durable evidence supports.
fn awaits_model_turn(active: &[MessageBlock]) -> bool {
    matches!(
        active.last(),
        Some(MessageBlock::User(user)) if user.kind == InboundKind::Message
    )
}

/// The honest canonical result of a tool execution whose external outcome the
/// runtime does not know.
fn interrupted_result() -> ToolExecutionResult {
    ToolExecutionResult {
        status: ToolExecutionStatus::Interrupted,
        content: Vec::new(),
        duration_ms: 0,
        exit_code: None,
        artifacts: Vec::new(),
        truncation: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::types::EVENT_SCHEMA_VERSION;
    use crate::model::finish::ModelFinishReason;
    use crate::runtime::identity::{
        AttemptId, ConversationId, EventId, MessageId, RequestId, ToolCallId, ToolId,
    };

    fn conversation() -> ConversationId {
        ConversationId::new("conv-fold")
    }

    fn attempt(ordinal: u64) -> AttemptId {
        AttemptId::for_conversation(&conversation(), ordinal)
    }

    fn envelope(event: RuntimeEvent, attempt_id: Option<AttemptId>) -> RuntimeEventEnvelope {
        // The fold never reads the event identity; a stable fixture id is
        // sufficient for the state-machine regressions.
        RuntimeEventEnvelope {
            schema_version: EVENT_SCHEMA_VERSION,
            event_id: EventId::new("evt"),
            sequence: 0,
            conversation_id: conversation(),
            attempt_id,
            turn_id: None,
            timestamp: Utc::now(),
            event,
        }
    }

    fn base_evidence() -> RecoveryEvidence {
        RecoveryEvidence {
            conversation_id: conversation(),
            active: Vec::new(),
            pending: Vec::new(),
            unsettled_attempts: BTreeMap::new(),
            tool_repairs: BTreeMap::new(),
            unsettled_background: Vec::new(),
            unsettled_subagents: Vec::new(),
            assistant_attempts: BTreeMap::new(),
            active_ids: std::collections::BTreeSet::new(),
            highest_attempt_ordinal: None,
            highest_background_ordinal: 0,
            highest_subagent_ordinal: 0,
            saw_any_attempt: false,
        }
    }

    fn fold_all(evidence: &mut RecoveryEvidence, events: &[RuntimeEventEnvelope]) {
        let mut background = BTreeMap::new();
        let mut subagents = BTreeMap::new();
        for envelope in events {
            evidence.fold(envelope, &mut background, &mut subagents);
        }
        evidence.unsettled_background = background.into_values().collect();
        evidence.unsettled_subagents = subagents.into_values().collect();
    }

    fn started(attempt_id: AttemptId) -> RuntimeEventEnvelope {
        envelope(
            RuntimeEvent::AttemptStarted {
                attempt_id: attempt_id.clone(),
            },
            Some(attempt_id),
        )
    }

    fn tool_started(attempt_id: AttemptId, call: &str) -> RuntimeEventEnvelope {
        envelope(
            RuntimeEvent::ToolExecutionStarted {
                tool_call_id: ToolCallId::new(call),
                tool_id: ToolId::new("tool-a"),
            },
            Some(attempt_id),
        )
    }

    /// The fold is a bounded working set: a fully settled tool lifecycle —
    /// start, outcome, canonical `ToolResult`, attempt terminal — leaves no
    /// evidence behind, so complete history is never materialized.
    #[test]
    fn a_settled_tool_lifecycle_leaves_no_evidence() {
        let a = attempt(0);
        let mut evidence = base_evidence();
        fold_all(
            &mut evidence,
            &[
                started(a.clone()),
                tool_started(a.clone(), "call-1"),
                envelope(
                    RuntimeEvent::ToolExecutionCompleted {
                        tool_call_id: ToolCallId::new("call-1"),
                        tool_id: ToolId::new("tool-a"),
                        result: interrupted_result(),
                    },
                    Some(a.clone()),
                ),
                envelope(
                    RuntimeEvent::ToolMessageCommitted {
                        message_id: MessageId::new("a-tool-call-1"),
                        tool_call_id: ToolCallId::new("call-1"),
                    },
                    Some(a.clone()),
                ),
                envelope(
                    RuntimeEvent::AttemptCompleted {
                        attempt_id: a.clone(),
                        finish_reason: ModelFinishReason::Stop,
                    },
                    Some(a),
                ),
            ],
        );
        assert!(
            evidence.unsettled_attempts.is_empty(),
            "no unsettled attempt"
        );
        assert!(
            evidence.tool_repairs.is_empty(),
            "no retained tool repair evidence"
        );
    }

    /// The Finding-B prefix at the fold level: a recovery-generated
    /// `ToolMessageCommitted` settles the canonical repair — releasing the
    /// per-call repair evidence — while the owning attempt's **summary**
    /// keeps the external-start evidence for the non-terminal attempt.
    #[test]
    fn recovery_repair_keeps_external_evidence_for_nonterminal_attempt() {
        let a = attempt(0);
        let assistant_id = MessageId::new("assistant-1");
        let mut evidence = base_evidence();
        evidence.active = vec![crate::message::types::MessageBlock::Assistant(
            crate::message::types::AssistantMessageBlock {
                id: assistant_id.clone(),
                content: Vec::new(),
            },
        )];
        evidence.active_ids.insert(assistant_id.clone());
        fold_all(
            &mut evidence,
            &[
                started(a.clone()),
                envelope(
                    RuntimeEvent::AssistantMessageCommitted {
                        message_id: assistant_id.clone(),
                    },
                    Some(a.clone()),
                ),
                tool_started(a.clone(), "call-1"),
                // The recovery repair commit: canonical ToolResult exists,
                // envelope carries no attempt identity.
                envelope(
                    RuntimeEvent::ToolMessageCommitted {
                        message_id: MessageId::new("assistant-1-recovered-tool-call-1"),
                        tool_call_id: ToolCallId::new("call-1"),
                    },
                    None,
                ),
            ],
        );
        assert_eq!(
            evidence.unsettled_attempts.len(),
            1,
            "attempt still non-terminal"
        );
        let entry = evidence
            .unsettled_attempts
            .get(&a)
            .expect("the attempt summary survives the canonical repair");
        assert!(
            entry.tools == ToolExternalSummary::UnknownIrreversible,
            "the summary still proves external execution started with an unknown outcome"
        );
        assert!(
            evidence.tool_repairs.is_empty(),
            "the canonical repair released the per-call repair evidence"
        );
    }

    /// A resolved model request never returns to `NeverStarted`: the fold
    /// transition is monotonic, so `ModelRequestStarted` +
    /// `ModelRequestCompleted` can never be read back as \"nothing started\".
    #[test]
    fn a_resolved_request_never_returns_to_never_started() {
        let a = attempt(0);
        let request_id = RequestId::new("req-1");
        let mut evidence = base_evidence();
        fold_all(
            &mut evidence,
            &[
                started(a.clone()),
                envelope(
                    RuntimeEvent::ModelRequestStarted {
                        request_id: request_id.clone(),
                        model: "model-x".to_owned(),
                    },
                    Some(a.clone()),
                ),
                envelope(
                    RuntimeEvent::ModelRequestCompleted {
                        finish_reason: ModelFinishReason::Stop,
                        usage: None,
                    },
                    Some(a),
                ),
            ],
        );
        let attempt = evidence
            .unsettled_attempts
            .values()
            .next()
            .expect("the attempt is still unresolved");
        assert_eq!(
            attempt.request,
            ExternalRequestLifecycle::StartedOutcomeKnown {
                request_id: Some(request_id),
                outcome: RequestOutcome::Completed,
            }
        );
        assert_ne!(
            attempt.request,
            ExternalRequestLifecycle::NeverStarted,
            "resolved outcome is never 'never started'"
        );
    }

    /// The bounded-working-set correction at the fold level: one non-terminal
    /// attempt that fully canonicalized **1000** tool calls across as many
    /// turns retains exactly one bounded attempt summary and **zero**
    /// detailed per-call repair evidence — the recovery hot detail does not
    /// scale with the number of settled calls.
    #[test]
    fn a_long_settled_fold_retains_bounded_repair_evidence() {
        const CALLS: usize = 1000;
        let a = attempt(0);
        let mut evidence = base_evidence();
        let mut events = vec![started(a.clone())];
        for call in 0..CALLS {
            events.push(envelope(
                RuntimeEvent::ToolExecutionStarted {
                    tool_call_id: ToolCallId::new(format!("call-{call}")),
                    tool_id: ToolId::new("tool-a"),
                },
                Some(a.clone()),
            ));
            events.push(envelope(
                RuntimeEvent::ToolExecutionCompleted {
                    tool_call_id: ToolCallId::new(format!("call-{call}")),
                    tool_id: ToolId::new("tool-a"),
                    result: interrupted_result(),
                },
                Some(a.clone()),
            ));
            events.push(envelope(
                RuntimeEvent::ToolMessageCommitted {
                    message_id: MessageId::new(format!("assistant-{call}-tool-{call}")),
                    tool_call_id: ToolCallId::new(format!("call-{call}")),
                },
                Some(a.clone()),
            ));
        }
        fold_all(&mut evidence, &events);
        assert_eq!(
            evidence.unsettled_attempts.len(),
            1,
            "1000 settled calls still leave exactly one bounded attempt summary"
        );
        assert!(
            evidence.tool_repairs.is_empty(),
            "1000 fully canonicalized calls retain zero detailed repair evidence"
        );
        let summary = &evidence
            .unsettled_attempts
            .get(&a)
            .expect("the attempt summary")
            .tools;
        assert!(
            summary == &ToolExternalSummary::AllOutcomesKnown,
            "the summary proves external tool work happened with all outcomes known"
        );
    }

    /// A terminal attempt keeps its non-canonical tool evidence (Class D) for
    /// repair, and drops it the moment its canonical result commits.
    #[test]
    fn attempt_terminal_keeps_class_d_evidence_then_drops_it_on_repair() {
        let a = attempt(0);
        let mut evidence = base_evidence();
        fold_all(
            &mut evidence,
            &[
                started(a.clone()),
                tool_started(a.clone(), "call-1"),
                envelope(
                    RuntimeEvent::AttemptFailed {
                        attempt_id: a.clone(),
                        error: AttemptFailure::Runtime {
                            error: RuntimeError::Internal {
                                message: "batch commit failed".to_owned(),
                            },
                        },
                    },
                    Some(a.clone()),
                ),
            ],
        );
        assert!(
            evidence
                .tool_repairs
                .contains_key(&(a.clone(), ToolCallId::new("call-1"))),
            "a started call of a terminal attempt stays repairable (Class D)"
        );
        // The recovery repair commits the canonical result; the owning
        // attempt is already terminal, so the entry resolves immediately.
        fold_all(
            &mut evidence,
            &[envelope(
                RuntimeEvent::ToolMessageCommitted {
                    message_id: MessageId::new("assistant-1-recovered-tool-call-1"),
                    tool_call_id: ToolCallId::new("call-1"),
                },
                Some(a.clone()),
            )],
        );
        assert!(
            evidence.tool_repairs.is_empty(),
            "resolved tool evidence of a settled attempt is dropped"
        );
    }
}
