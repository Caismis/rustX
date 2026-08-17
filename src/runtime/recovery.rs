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
//! and therefore, twice over:
//!
//! ```text
//! exact historical reconstruction  !=  safe replay permission
//! external outcome unknown         !=  retry
//! ```
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
//! state: non-terminal attempts (at most one by the admission invariant), the
//! in-flight model request of such an attempt, tool executions whose canonical
//! `ToolResult` is not committed (one finite foreground batch), and background
//! executions whose terminal publication is not committed (bounded by runtime
//! background policy). A resolved entry is dropped from the fold the moment
//! its resolving fact is read, so complete history is never materialized as
//! `Vec<RuntimeEvent>` or `Vec<RequestSnapshot>`.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};

use crate::conversation::{RecoverySafetyError, recovery_safety};
use crate::durable::{ConversationStore, ConversationStoreError, PendingInboundItem};
use crate::events::types::{AttemptFailure, RuntimeEvent, RuntimeEventEnvelope};
use crate::message::types::{AssistantContentBlock, InboundKind, MessageBlock, ToolMessageBlock};
use crate::runtime::identity::{
    AttemptId, ConversationId, EventId, MessageId, RequestId, ToolCallId, ToolExecutionId, ToolId,
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

/// What durable evidence says about one non-terminal attempt.
///
/// The entry exists only while the attempt is unresolved; the attempt's
/// terminal fact removes it from the fold.
#[derive(Debug, Clone, PartialEq, Eq)]
struct AttemptEvidence {
    /// The one model request whose start committed durably and whose outcome
    /// is not durably known.
    open_request: Option<RequestId>,
}

/// What durable evidence says about one tool execution whose canonical
/// `ToolResult` is not committed.
#[derive(Debug, Clone, PartialEq)]
enum ToolEvidence {
    /// `ToolExecutionStarted` committed; no outcome fact followed. The
    /// external outcome is **unknown**.
    StartedOutcomeUnknown {
        /// The executed tool.
        tool_id: ToolId,
    },
    /// A durable outcome fact exists; the canonical `ToolResult` message was
    /// simply never committed. Recovery uses this exact result, never an
    /// invented one.
    OutcomeKnown {
        /// The executed tool.
        tool_id: ToolId,
        /// The exact durable result.
        result: Box<ToolExecutionResult>,
    },
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
    /// Non-terminal attempts. The admission invariant permits at most one; the
    /// map is a map so a contract violation is *reported* rather than hidden.
    unsettled_attempts: BTreeMap<AttemptId, AttemptEvidence>,
    /// Tool executions with durable evidence and no committed `ToolResult`.
    unsettled_tools: BTreeMap<ToolCallId, ToolEvidence>,
    /// Background executions durably owned and not durably published.
    unsettled_background: Vec<BackgroundEvidence>,
    /// The highest conversation-scoped attempt ordinal that entered durable
    /// authority, terminal or not.
    highest_attempt_ordinal: Option<u64>,
    /// The highest background execution ordinal that entered durable
    /// authority, published or not.
    highest_background_ordinal: u64,
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
            unsettled_tools: BTreeMap::new(),
            unsettled_background: Vec::new(),
            highest_attempt_ordinal: None,
            highest_background_ordinal: 0,
            saw_any_attempt: false,
        };

        // The bounded fold. Each page is decoded, folded, and dropped; only
        // the unresolved working set survives a page boundary.
        let mut cursor = None;
        let mut background: BTreeMap<ToolExecutionId, BackgroundEvidence> = BTreeMap::new();
        loop {
            let page = store.read_events(cursor, RECOVERY_PAGE)?;
            if page.events.is_empty() {
                break;
            }
            for envelope in &page.events {
                evidence.fold(envelope, &mut background);
            }
            cursor = page.next_sequence;
            if cursor.is_none() {
                break;
            }
        }
        evidence.unsettled_background = background.into_values().collect();
        Ok(evidence)
    }

    /// Folds one durable event into the unresolved working set.
    #[allow(clippy::too_many_lines)] // One event vocabulary, one fold, one place.
    fn fold(
        &mut self,
        envelope: &RuntimeEventEnvelope,
        background: &mut BTreeMap<ToolExecutionId, BackgroundEvidence>,
    ) {
        match &envelope.event {
            RuntimeEvent::AttemptStarted { attempt_id } => {
                self.note_attempt(attempt_id);
                self.unsettled_attempts
                    .entry(attempt_id.clone())
                    .or_insert(AttemptEvidence { open_request: None });
            }
            RuntimeEvent::AttemptCompleted { attempt_id, .. }
            | RuntimeEvent::AttemptCancelled { attempt_id, .. }
            | RuntimeEvent::AttemptTimedOut { attempt_id }
            | RuntimeEvent::AttemptLimitExceeded { attempt_id, .. }
            | RuntimeEvent::AttemptFailed { attempt_id, .. } => {
                self.note_attempt(attempt_id);
                // A durable terminal is absorbing: the attempt leaves the
                // unresolved working set and never returns to it.
                self.unsettled_attempts.remove(attempt_id);
            }
            RuntimeEvent::ModelRequestStarted { request_id, .. } => {
                if let Some(attempt) = self.current_attempt_mut(envelope) {
                    attempt.open_request = Some(request_id.clone());
                }
            }
            RuntimeEvent::ModelRequestCompleted { .. }
            | RuntimeEvent::ModelRequestFailed { .. } => {
                if let Some(attempt) = self.current_attempt_mut(envelope) {
                    // The provider outcome is durably known, whatever it was.
                    attempt.open_request = None;
                }
            }
            RuntimeEvent::ToolExecutionStarted {
                tool_call_id,
                tool_id,
            } => {
                self.unsettled_tools.insert(
                    tool_call_id.clone(),
                    ToolEvidence::StartedOutcomeUnknown {
                        tool_id: tool_id.clone(),
                    },
                );
            }
            RuntimeEvent::ToolExecutionCompleted {
                tool_call_id,
                tool_id,
                result,
            } => {
                self.unsettled_tools.insert(
                    tool_call_id.clone(),
                    ToolEvidence::OutcomeKnown {
                        tool_id: tool_id.clone(),
                        result: Box::new(result.clone()),
                    },
                );
            }
            RuntimeEvent::ToolExecutionFailed {
                tool_call_id,
                tool_id,
                error,
            } => {
                self.unsettled_tools.insert(
                    tool_call_id.clone(),
                    ToolEvidence::OutcomeKnown {
                        tool_id: tool_id.clone(),
                        result: Box::new(ToolExecutionResult {
                            status: ToolExecutionStatus::Failed {
                                error: error.clone(),
                            },
                            content: Vec::new(),
                            duration_ms: 0,
                            exit_code: None,
                            artifacts: Vec::new(),
                            truncation: None,
                        }),
                    },
                );
            }
            RuntimeEvent::ToolMessageCommitted { tool_call_id, .. } => {
                // The canonical `ToolResult` exists: this call is settled and
                // leaves the unresolved working set.
                self.unsettled_tools.remove(tool_call_id);
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
    /// resend and **no** automatic re-execution.
    IndeterminateExternalOutcome {
        /// The interrupted attempt.
        attempt_id: AttemptId,
        /// The started request whose provider outcome is unknown, if any.
        model_request: Option<RequestId>,
        /// The started tool calls whose external outcome is unknown.
        tool_calls: Vec<ToolCallId>,
    },
    /// **Class D.** Every durable attempt already carries its one terminal
    /// fact. The state is absorbing: recovery adds no second terminal, and
    /// repeated restarts change nothing.
    AlreadyTerminal,
}

/// The recovery classification of one detached background execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackgroundRecoveryClass {
    /// The durably owned execution.
    pub evidence: BackgroundEvidence,
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
    /// The missing canonical `ToolResult` siblings, grouped by their owning
    /// Assistant message, in canonical model-call order.
    tool_repairs: Vec<ToolTurnRepair>,
    resume: ResumeDisposition,
    next_attempt_ordinal: u64,
    highest_background_ordinal: u64,
    pending_inbound: usize,
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
    /// Whether the external outcome was durably unknown.
    indeterminate: bool,
}

impl RecoveryPlan {
    /// **Phase 2.** Classifies the durable evidence deterministically.
    ///
    /// Pure: the same evidence always yields the same plan.
    #[must_use]
    pub fn classify(evidence: &RecoveryEvidence) -> Self {
        let tool_repairs = Self::plan_tool_repairs(evidence);
        let attempt = Self::classify_attempt(evidence, &tool_repairs);
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
            tool_repairs,
            resume,
            next_attempt_ordinal: evidence.next_attempt_ordinal(),
            highest_background_ordinal: evidence.highest_background_ordinal,
            pending_inbound: evidence.pending.len(),
        }
    }

    fn classify_attempt(
        evidence: &RecoveryEvidence,
        repairs: &[ToolTurnRepair],
    ) -> AttemptRecoveryClass {
        let Some((attempt_id, attempt)) = evidence.unsettled_attempts.iter().next() else {
            return if evidence.saw_any_attempt {
                AttemptRecoveryClass::AlreadyTerminal
            } else {
                AttemptRecoveryClass::NotStarted
            };
        };
        // Every tool call whose external start committed and whose outcome is
        // not durably known, from both directions:
        //
        //   - the Surface-visible repair plan (the ordinary case), and
        //   - any unresolved `StartedOutcomeUnknown` fold entry, so a started
        //     call whose owning Assistant message is no longer active still
        //     counts as an indeterminate external effect.
        //
        // The union is the honest answer: indeterminacy is a property of the
        // external world, not of what the current Surface happens to show.
        let mut indeterminate: std::collections::BTreeSet<ToolCallId> = repairs
            .iter()
            .flat_map(|repair| repair.missing.iter())
            .filter(|missing| missing.indeterminate)
            .map(|missing| missing.call_id.clone())
            .collect();
        indeterminate.extend(
            evidence
                .unsettled_tools
                .iter()
                .filter(|(_, tool)| matches!(tool, ToolEvidence::StartedOutcomeUnknown { .. }))
                .map(|(call_id, _)| call_id.clone()),
        );
        let indeterminate_tools: Vec<ToolCallId> = indeterminate.into_iter().collect();
        if attempt.open_request.is_some() || !indeterminate_tools.is_empty() {
            AttemptRecoveryClass::IndeterminateExternalOutcome {
                attempt_id: attempt_id.clone(),
                model_request: attempt.open_request.clone(),
                tool_calls: indeterminate_tools,
            }
        } else {
            AttemptRecoveryClass::AdmittedWithoutExternalStart {
                attempt_id: attempt_id.clone(),
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
                missing.push(match evidence.unsettled_tools.get(&call.id) {
                    // The external effect started and no outcome is durably
                    // known: the strongest honest native status.
                    Some(ToolEvidence::StartedOutcomeUnknown { tool_id }) => MissingToolResult {
                        call_id: call.id.clone(),
                        tool_id: tool_id.clone(),
                        result: interrupted_result(),
                        indeterminate: true,
                    },
                    // The outcome *is* durably known; the canonical message
                    // simply never committed. The durable result is used
                    // verbatim — no invented body, no completion race.
                    Some(ToolEvidence::OutcomeKnown { tool_id, result }) => MissingToolResult {
                        call_id: call.id.clone(),
                        tool_id: tool_id.clone(),
                        result: (**result).clone(),
                        indeterminate: false,
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
                        indeterminate: false,
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
    /// fails.
    pub fn reconcile(
        self,
        store: &dyn ConversationStore,
        clock: &dyn RuntimeClock,
    ) -> Result<RecoveryReport, RecoveryError> {
        let mut committed = RecoveryReconciliation::default();

        // ---- 1. canonical tool-turn repair ----
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
            // One transaction: the incomplete turn becomes complete, or
            // nothing changes. A durable prefix of the sibling batch is never
            // observable.
            store.append_canonical_batch_with_events(&blocks, &events)?;
            committed
                .repaired_tool_results
                .extend(repair.missing.iter().map(|missing| missing.call_id.clone()));
        }

        // ---- 2. attempt recovery terminal ----
        match &self.attempt {
            AttemptRecoveryClass::AdmittedWithoutExternalStart { attempt_id } => {
                self.terminalize(store, attempt_id, clock.now(), &format!(
                    "the runtime restarted while attempt {attempt_id} was durably non-terminal; \
                     no model request and no tool execution had crossed a durable start commit, \
                     so no external side effect is outstanding"
                ))?;
                committed.attempt_terminal = Some(attempt_id.clone());
            }
            AttemptRecoveryClass::IndeterminateExternalOutcome {
                attempt_id,
                model_request,
                tool_calls,
            } => {
                let request = model_request.as_ref().map_or_else(
                    || "no model request was in flight".to_owned(),
                    |request| {
                        format!(
                            "model request {request} started and its provider outcome is unknown"
                        )
                    },
                );
                let tools = if tool_calls.is_empty() {
                    "no tool execution outcome is indeterminate".to_owned()
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
                self.terminalize(store, attempt_id, clock.now(), &format!(
                    "the runtime restarted while attempt {attempt_id} was durably non-terminal; \
                     {request}; {tools}. Nothing was resent and nothing was re-executed"
                ))?;
                committed.attempt_terminal = Some(attempt_id.clone());
            }
            AttemptRecoveryClass::NotStarted | AttemptRecoveryClass::AlreadyTerminal => {}
        }

        // ---- 3. background terminal publication ----
        for class in &self.background {
            let (draft, event) = crate::tools::background::recovery_terminal_publication(
                &self.conversation_id,
                &class.evidence.execution_id,
                &class.evidence.tool_name,
                clock.now(),
            );
            // The terminal Pending Inbound row and the
            // `BackgroundTerminalPublished` fact commit in one transaction,
            // exactly as the live settlement path does. The stable producer
            // correlation and the durable `background:{execution_id}` terminal
            // lifecycle together make this exactly-once across any number of
            // restarts.
            store.accept_inbound_with_event(draft, event)?;
            committed
                .background_terminals
                .push(class.evidence.execution_id.clone());
        }

        Ok(RecoveryReport {
            attempt: self.attempt,
            background: self.background,
            resume: self.resume,
            reconciliation: committed,
            next_attempt_ordinal: self.next_attempt_ordinal,
            highest_background_ordinal: self.highest_background_ordinal,
            pending_inbound: self.pending_inbound,
        })
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
    }
}

/// The observable result of one startup recovery.
#[derive(Debug, Clone, PartialEq)]
pub struct RecoveryReport {
    attempt: AttemptRecoveryClass,
    background: Vec<BackgroundRecoveryClass>,
    resume: ResumeDisposition,
    reconciliation: RecoveryReconciliation,
    next_attempt_ordinal: u64,
    highest_background_ordinal: u64,
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
