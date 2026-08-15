//! The deterministic agent execution loop (M3 + M4 context integration).
//!
//! The loop turns one canonical `ModelEvent` stream into an attempt
//! execution:
//!
//! ```text
//! input
//!  ↓
//! ConversationState (Message Ledger + Conversation Surface) + pending
//! FreshInboundTurn
//!  ↓
//! ContextEngine → ContextProjection (+ ephemeral Agent Status attachment)
//!  ↓
//! model request (projection + tools + continuation state)
//!  ↓
//! canonical model events
//!  ↓
//! message assembly + RuntimeEvent emission
//!  ↓
//! tool calls (if requested): resolve, execute, record
//!  ↓
//! TurnCompleted → safe boundary → one finite inbound mailbox drain
//!  ↓
//! continuation (or proactive compaction / compact-and-retry on overflow)
//!  ↓
//! exactly one terminal RuntimeEvent
//! ```
//!
//! Ownership: the loop owns execution semantics, message assembly, tool
//! execution, continuation state, cancellation observation, context
//! projection integration, fresh-inbound lifecycle, safe-boundary inbound
//! consumption, and the runtime event trace. The adapter owns provider
//! protocol translation and Agent Status wire placement only. No provider
//! protocol concept appears in this module.
//!
//! The M4 context path is mandatory: every normal `AgentExecution` is
//! constructed with a [`ContextRuntime`], and there is exactly one normal
//! execution model — no no-context/unbounded mode and no Agent Status
//! disable flag. Agent Status is composed whenever a pending
//! [`FreshInboundTurn`] exists and is consumed by the first successful model
//! invocation that observes it. The conversation inbound mailbox is owned by
//! the conversation tool runtime: the loop drains exactly
//! `tool_runtime.mailbox()` at every safe boundary, so background terminal
//! notifications always enter the same mailbox the Agent Loop drains.
//! Cancellation is a generic Agent Loop invariant for every execution:
//! observable cancellation is checked before every model turn begins.

use futures_util::StreamExt;

use crate::capabilities::AttemptCapabilityLease;
use crate::context::ContextRuntime;
use crate::context::engine::CompactionConstraints;
use crate::context::error::{ContextError, ContextErrorKind};
use crate::context::projection::ContextProjection;
use crate::context::tokens::ProviderObservedInput;
use crate::conversation::{CompactionRecord, ConversationError, ConversationState};
use crate::events::types::{AttemptFailure, AttemptOutcome, RuntimeEvent};
use crate::message::types::{AssistantMessageBlock, MessageBlock, ToolMessageBlock};
use crate::model::adapter::ModelEventStream;
use crate::model::error::{ModelError, ModelErrorKind};
use crate::model::event::ModelEvent;
use crate::model::finish::ModelFinishReason;
use crate::model::session::AttemptModelSnapshot;
use crate::model::types::{
    AgentStatusAttachment, ModelRequest, ModelUsage, SkillCatalogAttachment,
};
use crate::runtime::continuation::ProviderContinuationState;
use crate::runtime::identity::{AgentId, AttemptId, ConversationId, MessageId, ToolCallId, ToolId};
use crate::runtime::inbound::{FreshInboundTurn, InitialTurnTrigger, MailboxError};
use crate::runtime::types::{CancellationReason, RuntimeError};
use crate::tools::background::BackgroundDispatchOutcome;
use crate::tools::executor::{
    PreflightOutcome, PreparedInvocation, ProgressReporter, ToolExecutionContext, ToolRegistry,
};
use crate::tools::runtime::ConversationToolRuntime;
use crate::tools::types::{
    ToolCall, ToolConcurrencyPolicy, ToolExecutionResult, ToolExecutionStatus, ToolInvocation,
    ToolInvocationMode, ToolProgress,
};

use super::assembly::ModelEventAssembler;
use super::cancellation::AgentCancellation;
use super::observer::{AgentExecutionObserver, AgentStatusObservation};
use super::state::{ExecutionState, ExecutionStateMachine};

use chrono::{DateTime, Utc};
use chrono_tz::Tz;

/// The bounded M4 retry policy for `ContextWindowExceeded`.
///
/// This is the only retry policy the loop implements: one compaction, one
/// retry. No generic backoff, rate-limit, timeout, transport, or provider
/// fallback retry exists.
pub const MAX_CONTEXT_OVERFLOW_RETRIES_PER_MODEL_TURN: u32 = 1;

/// Everything the loop needs to know about one attempt.
#[derive(Debug, PartialEq)]
pub struct AgentExecutionRequest {
    /// The agent being executed.
    pub agent_id: AgentId,
    /// The conversation the attempt belongs to.
    pub conversation_id: ConversationId,
    /// The attempt identity reported by attempt-level events.
    pub attempt_id: AttemptId,
    /// The one canonical conversation state the attempt takes ownership of.
    ///
    /// Ownership transfers into the attempt: while the attempt runs it is
    /// the single mutable conversation authority, and settlement transfers
    /// the state back to the host through
    /// [`AgentExecutionResult::conversation`]. There is deliberately no
    /// clone-based `initial_messages` API, so two independently mutable
    /// conversation copies are not representable.
    pub conversation: ConversationState,
    /// The explicit execution trigger of the attempt's first model turn.
    ///
    /// Fresh inbound identity is explicit execution state, never inferred
    /// from message role or history shape. [`InitialTurnTrigger::FreshInbound`]
    /// makes Agent Status and fresh-inbound validation mandatory; omitting a
    /// status or a fresh turn is impossible, so the trigger can never
    /// silently suppress Agent Status.
    pub initial_turn_trigger: InitialTurnTrigger,
    /// The per-execution/conversation IANA timezone metadata used by the
    /// temporal Agent Status section, when known. The process/system local
    /// timezone is never consulted.
    pub timezone: Option<Tz>,
    /// The one immutable model ownership object of the attempt.
    ///
    /// The loop receives exactly this instead of independent
    /// mutable-looking model fields. Every model turn of the attempt — the
    /// first request, every tool→model continuation, every context-overflow
    /// retry, every proactive-compaction continuation, and every compaction
    /// summary — uses this frozen snapshot and never reads live session
    /// model state.
    pub model: AttemptModelSnapshot,
}

/// The deterministic result of one attempt execution.
///
/// The recorded [`RuntimeEvent`] trace is the authoritative execution
/// record; the platform-level outcome maps one-to-one with the single
/// terminal event, and the committed messages are the final conversation
/// state of the attempt. The terminal execution state is the state-machine
/// settlement that produced the terminal event: they always represent the
/// same settlement boundary.
///
/// `conversation` is the authoritative conversation state handed back to
/// the host: the Message Ledger holding every committed fact of the attempt
/// (drained inbound User messages, committed Assistant messages, committed Tool
/// messages, and any committed runtime compaction summary) plus the
/// Conversation Surface at its final revision.
#[derive(Debug, PartialEq)]
pub struct AgentExecutionResult {
    /// The executed attempt.
    pub attempt_id: AttemptId,
    /// The one-to-one platform outcome of the single terminal event.
    pub outcome: AttemptOutcome,
    /// The terminal state-machine settlement that produced the terminal
    /// event: [`ExecutionState::Completed`] for successful settlement and
    /// [`ExecutionState::Failed`] for failure and cancellation settlement.
    pub terminal_state: ExecutionState,
    /// The ordered runtime event trace, ending with exactly one terminal
    /// event.
    pub events: Vec<RuntimeEvent>,
    /// The authoritative conversation state, transferred back to the host.
    pub conversation: ConversationState,
}

impl AgentExecutionResult {
    /// The committed Message Ledger records of the settled attempt, in
    /// commit order.
    ///
    /// This is the explicit audit/read path of the settled conversation: it
    /// enumerates the Ledger for a caller that actually asked for the
    /// complete committed history. The engine's normal projection and
    /// compaction paths never enumerate it.
    #[must_use]
    pub fn messages(&self) -> &[MessageBlock] {
        self.conversation.ledger().audit_records()
    }

    /// The active ordered message identities of the settled Conversation
    /// Surface.
    #[must_use]
    pub fn active_ids(&self) -> &[MessageId] {
        self.conversation.active_ids()
    }
}

/// One agent attempt execution.
///
/// The loop borrows the model adapter, the immutable tool registry, the
/// attempt cancellation signal, and owns the attempt capability lease, the
/// mandatory M4 context runtime, the conversation tool runtime (whose
/// canonical mailbox the loop drains), the execution state machine, the
/// committed history, the retained continuation state, the pending fresh
/// inbound trigger, and the runtime event trace.
pub struct AgentExecution<'a> {
    request: AgentExecutionRequest,
    capability: AttemptCapabilityLease,
    cancellation: &'a AgentCancellation,
    tool_runtime: &'a ConversationToolRuntime,
    state: ExecutionStateMachine,
    /// The attempt's owned conversation state: the single mutable
    /// conversation authority for the attempt's lifetime.
    conversation: ConversationState,
    events: Vec<RuntimeEvent>,
    pending_continuation: Option<ProviderContinuationState>,
    /// The committed Assistant message that established the pending
    /// continuation, when one is pending.
    continuation_owner: Option<MessageId>,
    /// The pending fresh inbound turn: `Some` until a successful model
    /// invocation has observed it. One pending fresh inbound turn produces
    /// at most one Agent Status snapshot per request preparation.
    pending_fresh_inbound: Option<FreshInboundTurn>,
    context_runtime: ContextRuntime,
    observed: Option<ProviderObservedInput>,
    last_request_fingerprint: Option<u64>,
    /// The optional live observation seam: when attached, every emitted
    /// runtime fact, every committed canonical message, and every composed
    /// Agent Status is observed at its commit linearization point. The
    /// attempt-local ordered `events` trace remains the authoritative
    /// record regardless of attachment.
    observer: Option<&'a dyn AgentExecutionObserver>,
    /// Test-only control point parked at the turn-continuation boundary:
    /// after a completed turn (and all its mailbox drain/append work)
    /// returned "continue", before the generic cancellation check of the
    /// next model turn; never present outside `#[cfg(test)]`. The `Mutex`
    /// keeps an execution with an installed pause `Sync` (the pause holds
    /// a `std::sync::mpsc::Receiver`), so host-driven attempt tasks can be
    /// spawned across threads in tests.
    #[cfg(test)]
    continuation_pause: std::sync::Mutex<Option<test_sync::ContinuationBoundaryPause>>,
    turn: u32,
    terminal_emitted: bool,
}

/// How one model stream of a turn ended.
enum StreamTerminal {
    Completed {
        finish_reason: ModelFinishReason,
        usage: Option<ModelUsage>,
    },
    Failed {
        error: ModelError,
    },
}

/// One completed model invocation: the provisional message identity, the
/// assembler holding the provisional stream content, and the stream
/// terminal.
///
/// The three pieces travel together: an overflow retry replaces the whole
/// invocation, so provisional output and tool calls of the failed request
/// are never committed under the retry's message identity.
struct ModelInvocation {
    message_id: MessageId,
    assembler: ModelEventAssembler,
    terminal: StreamTerminal,
}

/// The committed result of one successful compaction: the derived record
/// plus the measurements the completion event reports.
struct CompletedCompaction {
    /// The derived record of the applied compaction.
    record: CompactionRecord,
    /// The pre-compaction measurement, preserving its provenance.
    tokens_before: crate::runtime::types::TokenMeasurement,
    /// The deterministic estimate of the rebuilt request context.
    estimated_tokens_after: u64,
}

/// The terminal outcome of the whole attempt.
enum Terminal {
    Completed { finish_reason: ModelFinishReason },
    Cancelled { reason: CancellationReason },
    Failed { failure: AttemptFailure },
}

impl<'a> AgentExecution<'a> {
    /// Creates an attempt execution over the given adapter, the owned attempt
    /// capability lease, the cancellation signal, the mandatory M4 context
    /// runtime, and the conversation tool runtime.
    ///
    /// The attempt capability lease is moved into the execution and pins the
    /// immutable capability snapshot (revision, `ToolRegistry` handle, Skill
    /// catalog, environment identities, and the effective `ToolEnvironment`)
    /// for the complete lifetime of this attempt: every model/tool cycle
    /// inside the attempt uses exactly that snapshot and never re-discovers
    /// Skills. Constructor failure drops the owned lease, and successful
    /// execution settlement drops it with the consumed execution. The
    /// execution cannot be constructed without a lease — there is no
    /// capability-free constructor.
    ///
    /// The conversation tool runtime binds the conversation identity, the
    /// canonical inbound mailbox, and the background registry together:
    /// the attempt must belong to the same conversation, otherwise the
    /// execution is rejected structurally. The loop drains exactly
    /// `tool_runtime.mailbox()` at every safe boundary, so background
    /// terminal notifications always enter the mailbox the Agent Loop
    /// drains.
    ///
    /// The context runtime is required: there is exactly one normal
    /// execution model — canonical history is always projected through the
    /// context engine, and Agent Status is composed whenever a pending fresh
    /// inbound turn exists. There is no no-context mode and no Agent Status
    /// disable flag.
    ///
    /// # Errors
    ///
    /// Returns [`MailboxError::ConversationMismatch`] when the request's
    /// conversation differs from the conversation tool runtime's
    /// conversation (and therefore its canonical mailbox).
    pub fn new(
        request: AgentExecutionRequest,
        capability: AttemptCapabilityLease,
        cancellation: &'a AgentCancellation,
        context_runtime: ContextRuntime,
        tool_runtime: &'a ConversationToolRuntime,
    ) -> Result<Self, MailboxError> {
        if tool_runtime.conversation_id() != &request.conversation_id {
            return Err(MailboxError::ConversationMismatch {
                expected: request.conversation_id.clone(),
                actual: tool_runtime.conversation_id().clone(),
            });
        }
        let snapshot = capability.snapshot();
        if snapshot.conversation_id() != tool_runtime.conversation_id()
            || snapshot.workspace_root() != tool_runtime.workspace().root()
        {
            return Err(MailboxError::CapabilityOwnershipMismatch {
                capability_conversation: snapshot.conversation_id().clone(),
                runtime_conversation: tool_runtime.conversation_id().clone(),
                capability_workspace: snapshot.workspace_root().to_path_buf(),
                runtime_workspace: tool_runtime.workspace().root().to_path_buf(),
            });
        }
        let mut request = request;
        let conversation = core::mem::take(&mut request.conversation);
        Ok(Self {
            conversation,
            request,
            capability,
            cancellation,
            tool_runtime,
            state: ExecutionStateMachine::new(),
            events: Vec::new(),
            pending_continuation: None,
            continuation_owner: None,
            pending_fresh_inbound: None,
            context_runtime,
            observed: None,
            last_request_fingerprint: None,
            observer: None,
            #[cfg(test)]
            continuation_pause: std::sync::Mutex::new(None),
            turn: 0,
            terminal_emitted: false,
        })
    }

    /// Attaches the live observation seam.
    ///
    /// The observer is read-only: it observes emitted runtime facts,
    /// committed canonical messages, and composed Agent Status at their
    /// commit linearization points, and it never influences execution. It
    /// must be attached before [`AgentExecution::run`] to observe the whole
    /// attempt; attaching is optional, and the attempt-local ordered event
    /// trace in the result is recorded regardless.
    pub fn observe(&mut self, observer: &'a dyn AgentExecutionObserver) {
        self.observer = Some(observer);
    }

    /// Runs the attempt to its single terminal outcome.
    ///
    /// The execution state machine is the settlement authority: the machine
    /// is settled (`complete()` for success, `fail()` for failure and
    /// cancellation) immediately before the single attempt terminal
    /// `RuntimeEvent` is emitted, so the terminal event and the terminal
    /// state represent the same settlement boundary.
    ///
    /// # Panics
    ///
    /// Panics only when the loop violates its own invariants (the state
    /// machine rejects the settlement, an attempt that never settles, or a
    /// terminal event that does not map to an outcome); these are
    /// unreachable by construction.
    pub async fn run(mut self) -> AgentExecutionResult {
        self.emit(RuntimeEvent::AttemptStarted {
            attempt_id: self.request.attempt_id.clone(),
        });
        let terminal = if let Err(error) = self.state.start() {
            Terminal::Failed {
                failure: AttemptFailure::Runtime { error },
            }
        } else {
            // The attempt's explicit fresh inbound trigger is pending until
            // the first successful model invocation observes it. A pure
            // continuation attempt has no pending trigger: the trigger makes
            // the execution mode explicit, so Agent Status can never be
            // silently suppressed.
            self.pending_fresh_inbound = match &self.request.initial_turn_trigger {
                InitialTurnTrigger::FreshInbound(fresh) => Some(fresh.clone()),
                InitialTurnTrigger::Continuation => None,
            };
            let mut terminal = None;
            while terminal.is_none() {
                // Generic Agent Loop cancellation checkpoint:
                // observable cancellation is checked before every model
                // turn begins — the first turn, every continuation
                // after a foreground tool turn, and every continuation
                // caused by a drained inbound batch. This is an
                // intentional pre-1.0 Agent Loop contract refinement:
                // mailbox attachment, mailbox contents, the context
                // runtime, and the provider protocol do not control
                // generic cancellation timing. When cancellation wins
                // here, no `TurnStarted`, no `ModelRequestStarted`, and
                // no adapter invocation happen for the next turn.
                //
                // The check never replaces a terminal outcome already
                // selected at a mailbox safe boundary: a successful
                // no-tool turn whose empty mailbox snapshot settled the
                // attempt as Completed exits this loop before the check
                // runs again, so a later cancellation or enqueue never
                // reopens or reclassifies the completed attempt.
                if self.cancellation.is_cancelled() {
                    terminal = Some(Terminal::Cancelled {
                        reason: self.cancellation.reason(),
                    });
                    break;
                }
                terminal = self.run_turn().await;
                // TEST-ONLY continuation boundary: the previous turn is
                // structurally complete (every tool result and every
                // mailbox drain/append of that turn is done) and the
                // loop is about to check cancellation again before the
                // next model turn. Tests park here to make cancellation
                // observable deterministically between turns.
                #[cfg(test)]
                if terminal.is_none()
                    && let Some(pause) = &self
                        .continuation_pause
                        .lock()
                        .expect("continuation pause lock")
                        .as_ref()
                {
                    pause.park_at_continuation_boundary();
                }
            }
            terminal.expect("the attempt must settle")
        };
        self.settle(&terminal);
        self.emit_terminal(&terminal);
        let terminal_event = self.events.last().expect("terminal event emitted");
        let outcome =
            AttemptOutcome::from_terminal_event(terminal_event).expect("terminal maps to outcome");
        AgentExecutionResult {
            attempt_id: self.request.attempt_id,
            outcome,
            terminal_state: self.state.state(),
            events: self.events,
            conversation: self.conversation,
        }
    }

    /// Settles the execution state machine for the computed terminal
    /// outcome, immediately before the terminal event is emitted.
    fn settle(&mut self, terminal: &Terminal) {
        let settlement = match terminal {
            Terminal::Completed { .. } => self.state.complete(),
            Terminal::Cancelled { .. } | Terminal::Failed { .. } => self.state.fail(),
        };
        settlement.expect("the execution state machine must accept the settlement");
    }

    /// Executes one turn: one model request, its tool calls, and their
    /// results. Returns the terminal outcome when the attempt settled.
    async fn run_turn(&mut self) -> Option<Terminal> {
        self.turn += 1;
        let assistant_message_id =
            MessageId::new(format!("{}-agent-{}", self.request.attempt_id, self.turn));
        self.emit(RuntimeEvent::TurnStarted);

        let request = match self.prepare_model_request().await {
            Ok(request) => request,
            Err(terminal) => return Some(terminal),
        };
        self.emit(RuntimeEvent::ModelRequestStarted {
            model: request.model().to_owned(),
        });
        let mut invocation = match self
            .consume_invocation(request, &assistant_message_id)
            .await
        {
            Ok(invocation) => invocation,
            Err(terminal) => return Some(terminal),
        };

        // M4 bounded compact-and-retry: a recoverable context overflow does
        // not settle the attempt. The execution state remains an active
        // model-running state; no state-machine settlement and no attempt
        // terminal event are produced between the overflow and the retry.
        //
        // The retry budget is per model turn: `overflow_retries` is
        // turn-local, so every turn is entitled to its own
        // `MAX_CONTEXT_OVERFLOW_RETRIES_PER_MODEL_TURN` retries and the
        // budget never persists across turns. The retry path is single-shot:
        // a retry that overflows again settles the attempt, so there is no
        // second retry inside any individual turn.
        let overflow_retries: u32 = 0;
        if let StreamTerminal::Failed { error } = &invocation.terminal
            && error.kind == ModelErrorKind::ContextWindowExceeded
            && overflow_retries < MAX_CONTEXT_OVERFLOW_RETRIES_PER_MODEL_TURN
        {
            let retry_number = overflow_retries + 1;
            let overflow_error = error.clone();
            match self
                .retry_after_overflow(&overflow_error, retry_number)
                .await
            {
                Ok(retry_invocation) => {
                    // The successful retry replaces the complete invocation:
                    // the provisional identity, the assembler (and therefore
                    // the provisional content and tool calls of the failed
                    // request), and the terminal.
                    invocation = retry_invocation;
                }
                Err(terminal) => return Some(terminal),
            }
        }

        match invocation.terminal {
            StreamTerminal::Failed { error } => Some(Terminal::Failed {
                failure: AttemptFailure::Model { error },
            }),
            StreamTerminal::Completed {
                finish_reason,
                usage,
            } => {
                self.complete_turn(
                    invocation.message_id,
                    finish_reason,
                    usage,
                    invocation.assembler,
                )
                .await
            }
        }
    }

    /// Settles one completed model stream: assembly, usage folding,
    /// continuation retention, message commit, tool execution, and the
    /// turn-completion event. Returns the terminal outcome when the attempt
    /// settled.
    async fn complete_turn(
        &mut self,
        assistant_message_id: MessageId,
        finish_reason: ModelFinishReason,
        usage: Option<ModelUsage>,
        assembler: ModelEventAssembler,
    ) -> Option<Terminal> {
        let turn_assembly = match assembler.finish(&finish_reason, usage) {
            Ok(assembly) => assembly,
            Err(error) => {
                return Some(Terminal::Failed {
                    failure: AttemptFailure::Runtime { error },
                });
            }
        };
        // The pending fresh inbound turn is consumed by the first successful
        // model invocation, including a successful ToolCalls response: the
        // model has already observed the inbound turn, so the following
        // tool-only continuation carries no Agent Status unless a new
        // mailbox batch is drained later.
        self.pending_fresh_inbound = None;
        // The model request completion is reported with the canonical
        // final usage: the terminal event's reported usage, else the
        // latest usage update, never a sum of snapshots.
        let reported_usage = turn_assembly.usage;
        self.emit(RuntimeEvent::ModelRequestCompleted {
            finish_reason: finish_reason.clone(),
            usage: reported_usage.clone(),
        });
        // A provider-reported input measurement applies only to the
        // exact projection the completed request used.
        if let Some(usage) = &reported_usage
            && let Some(fingerprint) = self.last_request_fingerprint.take()
        {
            self.observed = Some(ProviderObservedInput {
                fingerprint,
                input_tokens: usage.input_tokens,
            });
        }
        self.pending_continuation = turn_assembly.continuation;
        if self.pending_continuation.is_some() {
            self.continuation_owner = Some(assistant_message_id.clone());
        } else {
            self.continuation_owner = None;
        }
        let has_tool_calls = !turn_assembly.tool_calls.is_empty();
        if let Err(error) = self.state.model_finished(has_tool_calls) {
            return Some(Terminal::Failed {
                failure: AttemptFailure::Runtime { error },
            });
        }
        // M5 preflight boundary: every model-issued tool call of the turn
        // must resolve structurally (registry identity, execution-policy
        // resolution, runtime metadata extraction, business argument
        // validation) before the Assistant message is committed. An impossible
        // canonical identity mismatch or unregistered tool is a
        // runtime/model-stream contract failure and the Assistant message is
        // never committed. Business JSON Schema validation failures are
        // normal rejected result slots and do not fail the attempt.
        let preflight = match self.preflight_tool_calls(&turn_assembly.tool_calls) {
            Ok(outcomes) => outcomes,
            Err(error) => {
                return Some(Terminal::Failed {
                    failure: AttemptFailure::Runtime { error },
                });
            }
        };
        if let Err(error) =
            self.commit_assistant_message(&assistant_message_id, &turn_assembly.content)
        {
            return Some(Terminal::Failed {
                failure: AttemptFailure::Runtime {
                    error: RuntimeError::ContractViolation {
                        message: format!(
                            "the assembled Assistant message cannot be committed: {error}"
                        ),
                    },
                },
            });
        }
        if !has_tool_calls {
            self.emit(RuntimeEvent::TurnCompleted);
            // Safe boundary for a completed no-tool turn: the attempt may
            // settle only when the boundary snapshot observes no eligible
            // inbound work. A drained batch keeps the attempt running for
            // one further model turn, so a pending inbound message prevents
            // a successful Stop from settling before it is observed.
            return match self.safe_boundary_drain() {
                Ok(true) => None,
                Ok(false) => Some(Terminal::Completed { finish_reason }),
                Err(terminal) => Some(terminal),
            };
        }
        // The entire tool-result batch is structurally settled exactly once
        // inside `execute_tools` before this point returns: every logical
        // call of the committed batch receives exactly one canonical
        // attempt-facing result slot, committed in original model call
        // order. Attempt cancellation can still settle the attempt as
        // cancelled after the structurally complete result batch is
        // committed; no next model turn starts after cancellation.
        self.execute_tools(&turn_assembly.tool_calls, preflight)
            .await;
        if self.cancellation.is_cancelled() {
            return Some(Terminal::Cancelled {
                reason: self.cancellation.reason(),
            });
        }
        if let Err(error) = self.state.tools_finished() {
            return Some(Terminal::Failed {
                failure: AttemptFailure::Runtime { error },
            });
        }
        // Safe boundary after a structurally complete tool turn: every
        // foreground call of the turn executed and every ToolMessage was
        // committed before this point. One finite mailbox drain may attach
        // an inbound batch to the continuation; the drain never splits the
        // tool-result batch.
        self.safe_boundary_drain().err()
    }

    /// Preflights every model-issued tool call of the turn.
    ///
    /// An impossible canonical identity mismatch or unregistered tool is a
    /// runtime/model-stream contract failure; business schema violations are
    /// normal [`PreflightOutcome::Rejected`] result slots.
    fn preflight_tool_calls(
        &self,
        calls: &[ToolCall],
    ) -> Result<Vec<PreflightOutcome>, RuntimeError> {
        let mut outcomes = Vec::with_capacity(calls.len());
        for call in calls {
            match self.tool_registry().preflight(call) {
                Ok(outcome) => outcomes.push(outcome),
                Err(error) => {
                    return Err(match error {
                        crate::tools::executor::ToolPreflightError::UnknownTool { name } => {
                            RuntimeError::UnknownTool { name }
                        }
                        crate::tools::executor::ToolPreflightError::IdentityMismatch {
                            id,
                            name,
                        } => RuntimeError::ContractViolation {
                            message: format!(
                                "tool call identity mismatch: id {id} and name {name:?}                                      do not resolve to the same registered tool"
                            ),
                        },
                    });
                }
            }
        }
        Ok(outcomes)
    }

    /// The mailbox-specific safe boundary: exactly one finite inbound
    /// mailbox snapshot after the current turn is structurally complete.
    ///
    /// This function is mailbox-owned semantics only, separate from the
    /// generic Agent Loop cancellation checkpoint (which lives in `run()`
    /// before every model turn). The conversation tool runtime owns the one
    /// canonical mailbox of the conversation; the loop drains exactly that
    /// mailbox, so background terminal notifications always enter the same
    /// mailbox the Agent Loop drains. With no pending items the snapshot
    /// observes no mailbox state and the function returns `Ok(false)`.
    /// Cancellation wins before batch selection: when cancellation is
    /// already observable, no drain happens, all pending items stay in the
    /// mailbox, and the attempt settles cancelled. Otherwise one atomic
    /// drain is performed and, once drained, the complete batch is
    /// appended synchronously as distinct canonical `UserMessageBlock`
    /// values in inbound sequence order — the batch is never partially
    /// consumed and never requeued. The whole drained batch becomes one
    /// new [`FreshInboundTurn`] in sequence order, so the next model
    /// request receives exactly one Agent Status snapshot targeting the
    /// final drained message (the highest-sequence item). If cancellation
    /// becomes observable only after the append, the batch stays canonical
    /// and the generic pre-next-turn checkpoint prevents any further model
    /// turn.
    ///
    /// Returns `Ok(true)` when one complete batch was appended, `Ok(false)`
    /// when the snapshot observed an empty mailbox, and the attempt
    /// terminal when cancellation was observable before the snapshot.
    fn safe_boundary_drain(&mut self) -> Result<bool, Terminal> {
        let mailbox = self.tool_runtime.mailbox();
        if self.cancellation.is_cancelled() {
            return Err(Terminal::Cancelled {
                reason: self.cancellation.reason(),
            });
        }
        let Some(batch) = mailbox.drain() else {
            return Ok(false);
        };
        let mut message_ids = Vec::with_capacity(batch.items().len());
        for item in batch.into_items() {
            let block = MessageBlock::User(item.into_message());
            match self.commit_canonical(&block) {
                Ok(message_id) => message_ids.push(message_id),
                Err(error) => {
                    return Err(Terminal::Failed {
                        failure: AttemptFailure::Runtime {
                            error: RuntimeError::ContractViolation {
                                message: format!(
                                    "a drained inbound message cannot be committed: {error}"
                                ),
                            },
                        },
                    });
                }
            }
        }
        let fresh = FreshInboundTurn::new(message_ids).map_err(|error| Terminal::Failed {
            failure: AttemptFailure::Runtime {
                error: RuntimeError::ContractViolation {
                    message: format!(
                        "a drained mailbox batch cannot form a fresh inbound turn: {error}"
                    ),
                },
            },
        })?;
        self.pending_fresh_inbound = Some(fresh);
        Ok(true)
    }

    /// Builds the canonical request of the next model invocation.
    ///
    /// This is the integration point immediately before every agent
    /// `ModelRequest`: the current Surface flows through the context engine
    /// into a projection, and the projection is compiled into the request
    /// messages. The pending fresh inbound turn
    /// (when one exists) is composed into exactly one Agent Status snapshot
    /// for this request preparation, and that exact snapshot is reused
    /// throughout proactive compaction planning and application. Proactive
    /// automatic compaction runs when the projected input reaches the soft
    /// input limit.
    async fn prepare_model_request(&mut self) -> Result<ModelRequest, Terminal> {
        if self.cancellation.is_cancelled() {
            return Err(Terminal::Cancelled {
                reason: self.cancellation.reason(),
            });
        }
        // One request preparation composes one status snapshot; the retry
        // after a ContextWindowExceeded begins a new preparation and
        // composes a fresh one.
        let status = match self.compose_status() {
            Ok(status) => status,
            Err(error) => return Err(Self::context_failure_terminal(&error)),
        };
        let projection = match self.current_projection(status.as_ref()) {
            Ok(projection) => projection,
            Err(terminal) => return Err(terminal),
        };
        let budgets = self.compaction_budgets();
        let should_compact = match self
            .context_runtime
            .engine
            .should_compact(&projection, budgets.primary_output_budget)
        {
            Ok(value) => value,
            Err(error) => return Err(Self::context_failure_terminal(&error)),
        };
        if should_compact {
            let must_cover = self.continuation_owner.clone();
            let fresh = self.pending_fresh_inbound.clone();
            // `perform_compaction` owns the post-surface-rewrite
            // invalidation of the opaque provider continuation; no caller
            // clears it a second time.
            match self
                .perform_compaction(must_cover.as_ref(), fresh.as_ref(), status.as_ref(), None)
                .await
            {
                Ok(()) => {}
                Err(terminal) => return Err(terminal),
            }
        }
        self.context_model_request(status.as_ref())
    }

    /// Composes the Agent Status attachment of the pending fresh inbound
    /// turn, sampling the runtime clock exactly once.
    ///
    /// With no pending fresh inbound turn there is no Agent Status. With a
    /// pending turn, the turn is validated against canonical history and the
    /// final message's persisted timestamp drives `inbound_message_time`;
    /// the composer produces the structured sections and the canonical
    /// renderer produces the attachment text.
    ///
    /// # Errors
    ///
    /// Returns a context error for a fresh-inbound contract violation
    /// (`MalformedHistory`) or a failing status section provider
    /// (`StatusFailed`).
    fn compose_status(&self) -> Result<Option<AgentStatusAttachment>, ContextError> {
        let Some(fresh) = &self.pending_fresh_inbound else {
            return Ok(None);
        };
        let active = self.conversation.active_messages().map_err(|error| {
            ContextError::new(ContextErrorKind::MalformedHistory, error.to_string())
        })?;
        fresh.validate_against(&active).map_err(|error| {
            ContextError::new(
                ContextErrorKind::MalformedHistory,
                format!("pending fresh inbound turn is inconsistent: {error}"),
            )
        })?;
        let target_message_id = fresh.last_message_id().clone();
        let inbound_message_time = self.inbound_time_of(&target_message_id).ok_or_else(|| {
            ContextError::new(
                ContextErrorKind::MalformedHistory,
                format!(
                    "pending fresh inbound message {target_message_id} has no persisted timestamp"
                ),
            )
        })?;
        let context = crate::context::status::AgentStatusRenderContext {
            inbound_message_time,
            timezone: self.request.timezone,
            background: self.tool_runtime.background().active_snapshot(),
        };
        let status = self.context_runtime.status_composer.compose(&context)?;
        if let Some(observer) = self.observer {
            observer.observe_status(&AgentStatusObservation {
                attempt_id: self.request.attempt_id.clone(),
                turn: self.turn,
                target_message_id: target_message_id.clone(),
                status: status.clone(),
            });
        }
        Ok(Some(AgentStatusAttachment {
            target_message_id,
            rendered: crate::context::status::render_agent_status(&status),
        }))
    }

    /// The persisted timestamp of one committed inbound message.
    fn inbound_time_of(&self, message_id: &MessageId) -> Option<DateTime<Utc>> {
        match self.conversation.ledger().get(message_id) {
            Some(MessageBlock::User(user)) => user.timestamp,
            _ => None,
        }
    }

    /// Builds the model request from the current Conversation Surface
    /// projection.
    fn context_model_request(
        &mut self,
        status: Option<&AgentStatusAttachment>,
    ) -> Result<ModelRequest, Terminal> {
        let projection = self.current_projection(status)?;
        self.last_request_fingerprint = Some(projection.fingerprint());
        Ok(self.model_request_from_projection(&projection))
    }

    /// The current finite projection of the Conversation Surface, or the
    /// terminal the attempt must settle with when the context plane failed.
    fn current_projection(
        &self,
        agent_status: Option<&AgentStatusAttachment>,
    ) -> Result<ContextProjection, Terminal> {
        self.context_runtime
            .engine
            .build_projection(
                &self.conversation,
                &self.tool_registry().model_definitions(),
                self.observed.as_ref(),
                agent_status,
                self.capability.snapshot().skill_catalog_attachment(),
            )
            .map_err(|error| Self::context_failure_terminal(&error))
    }

    /// The exact Skill catalog attachment of the pinned capability
    /// snapshot. The catalog is immutable for the attempt, so the exact
    /// attachment participates on both sides of every compaction progress
    /// comparison.
    fn skill_catalog(&self) -> Option<&SkillCatalogAttachment> {
        self.capability.snapshot().skill_catalog_attachment()
    }

    fn compaction_budgets(&self) -> crate::context::CompactionBudgets {
        self.context_runtime.compaction_budgets
    }

    /// The immutable `ToolRegistry` handle of the pinned capability snapshot.
    fn tool_registry(&self) -> &ToolRegistry {
        self.capability.snapshot().tool_registry()
    }

    /// The `AttemptFailed` terminal of a context-plane failure that occurred
    /// while preparing model context **before any compaction began**: an
    /// invalid pending fresh-inbound state discovered during status
    /// composition or projection preparation, a failing Agent Status section
    /// provider, or a projection preparation failure that is not itself a
    /// compaction operation. These are never mislabeled as compaction
    /// failures: [`RuntimeError::ContextCompactionFailed`] is reserved for an
    /// actual proactive compaction pipeline failure.
    fn context_failure_terminal(error: &ContextError) -> Terminal {
        Terminal::Failed {
            failure: AttemptFailure::Runtime {
                error: RuntimeError::ContextPreparationFailed {
                    message: error.message.clone(),
                },
            },
        }
    }

    /// Runs one compaction: plan, summarize, verify progress and fit, and
    /// commit the canonical summary plus the Surface rewrite.
    ///
    /// `overflow` distinguishes the two callers: a proactive compaction
    /// failure settles as `AttemptFailed(Runtime(ContextCompactionFailed))`,
    /// while a compaction after a context overflow preserves the normalized
    /// overflow as the final model failure (`AttemptFailed(Model(overflow))`)
    /// with the compaction diagnostic carried by `CompactionFailed.error`.
    ///
    /// The compaction planning receives the pending fresh inbound turn (so
    /// unobserved fresh inbound can never be retired) and the exact Agent
    /// Status attachment of this request preparation (so hard-fit estimates
    /// include the status itself).
    ///
    /// Cancellation is observed before the compaction, raced (biased)
    /// against the pending summary, checked again before the semantic
    /// commit, and checked again before any retry by the callers: once
    /// cancellation is observable, no summary, no Ledger append, no Surface
    /// rewrite, and no retry progress may begin, and the pending summary
    /// future is dropped.
    ///
    /// This is also the **one** ownership path that invalidates the opaque
    /// provider continuation: a successful incompatible Surface rewrite
    /// invalidates it exactly once, immediately after the semantic commit.
    /// A failed or cancelled compaction never clears it.
    async fn perform_compaction(
        &mut self,
        must_cover_through: Option<&MessageId>,
        fresh_inbound: Option<&FreshInboundTurn>,
        agent_status: Option<&AgentStatusAttachment>,
        overflow: Option<&ModelError>,
    ) -> Result<(), Terminal> {
        if self.cancellation.is_cancelled() {
            return Err(Terminal::Cancelled {
                reason: self.cancellation.reason(),
            });
        }
        self.emit(RuntimeEvent::CompactionStarted);
        match self
            .run_compaction(must_cover_through, fresh_inbound, agent_status)
            .await
        {
            Ok(completed) => {
                // The semantic commit already happened: the summary is a
                // Ledger fact and the new Surface revision exists. The
                // opaque provider continuation is now known to be
                // incompatible, and this is the single place that discards
                // it. The observed provider measurement belonged to the old
                // request context and is dropped with it.
                self.pending_continuation = None;
                self.continuation_owner = None;
                self.observed = None;
                self.emit(RuntimeEvent::CompactionCompleted {
                    generation: completed.record.generation,
                    summary_message_id: completed.record.summary_message_id.clone(),
                    surface_revision: completed.record.surface_revision,
                    tokens_before: completed.tokens_before,
                    estimated_tokens_after: completed.estimated_tokens_after,
                });
            }
            // Cancellation never becomes a compaction failure: no
            // `CompactionFailed` event is emitted and the attempt settles
            // cancelled.
            Err(error) if error.kind == ContextErrorKind::Cancelled => {
                return Err(Terminal::Cancelled {
                    reason: self.cancellation.reason(),
                });
            }
            Err(error) => return Err(self.compaction_failure(&error, overflow)),
        }
        Ok(())
    }

    /// The cancellation-aware compaction pipeline: plan, summarize, verify
    /// progress, fit-check, and commit.
    ///
    /// The **semantic commit / linearization point** of compaction is the
    /// single call to `ConversationState::commit_compaction`. Before it the
    /// old Ledger, the old Surface, and the old continuation semantics are
    /// authoritative; after it the canonical
    /// `User(Runtime / CompactionSummary)` message exists in the Ledger, a
    /// new Surface revision exists in which that summary replaces the
    /// selected active span, every covered Ledger fact remains intact and
    /// addressable, and the provider continuation is known to be
    /// incompatible.
    ///
    /// Cancellation is observed before the compaction, raced (biased)
    /// against the pending summary, and checked again immediately before the
    /// semantic commit: once cancellation is observable, no summary, no
    /// canonical summary append, and no Surface rewrite happen, so no
    /// half-committed state can exist.
    async fn run_compaction(
        &mut self,
        must_cover_through: Option<&MessageId>,
        fresh_inbound: Option<&FreshInboundTurn>,
        agent_status: Option<&AgentStatusAttachment>,
    ) -> Result<CompletedCompaction, ContextError> {
        if self.cancellation.is_cancelled() {
            return Err(ContextError::new(
                ContextErrorKind::Cancelled,
                "compaction cancelled before it began",
            ));
        }
        let tools = self.tool_registry().model_definitions();
        let projection = self.context_runtime.engine.build_projection(
            &self.conversation,
            &tools,
            self.observed.as_ref(),
            agent_status,
            self.skill_catalog(),
        )?;
        let budgets = self.compaction_budgets();
        let plan = self.context_runtime.engine.plan_compaction(
            &self.conversation,
            &projection,
            &tools,
            budgets,
            &CompactionConstraints {
                must_cover_through,
                fresh_inbound,
            },
        )?;
        let summary_request = plan.summary_request();
        let summary = tokio::select! {
            biased;
            () = self.cancellation.cancelled() => {
                return Err(ContextError::new(
                    ContextErrorKind::Cancelled,
                    "compaction cancelled while summarizing",
                ));
            }
            result = self
                .context_runtime
                .summarizer
                .summarize(summary_request, self.cancellation.model_cancellation()) => result,
        };
        let summary_text = summary?;
        // Cancellation after the summary returned but before the semantic
        // commit: nothing is appended, nothing is rewritten, no completion
        // is emitted, and the old state stays authoritative.
        if self.cancellation.is_cancelled() {
            return Err(ContextError::new(
                ContextErrorKind::Cancelled,
                "compaction cancelled before the semantic commit",
            ));
        }
        let (commit, projection) = self.context_runtime.engine.prepare_compaction(
            &self.conversation,
            &self.request.conversation_id,
            &plan,
            &summary_text,
            &tools,
        )?;
        // The rebuilt projection must fit under the soft input limit; if
        // retained context and the actual summary cannot fit, fail
        // explicitly before anything is committed.
        match self
            .context_runtime
            .engine
            .fits_under_soft_limit(&projection, budgets.primary_output_budget)
        {
            Ok(true) => {}
            Ok(false) => {
                return Err(ContextError::new(
                    ContextErrorKind::CannotFit,
                    "the compacted surface still exceeds the soft input limit",
                ));
            }
            Err(error) => return Err(error),
        }
        // The semantic commit point.
        let summary_block = MessageBlock::User(commit.summary.clone());
        let record = self
            .conversation
            .commit_compaction(commit)
            .map_err(|error| ContextError::new(ContextErrorKind::Internal, error.to_string()))?;
        // The committed runtime summary is a canonical Ledger fact, observed
        // at exactly the commit linearization point like every other commit.
        if let Some(observer) = self.observer {
            observer.observe_committed(&self.request.attempt_id, &summary_block);
        }
        Ok(CompletedCompaction {
            record,
            tokens_before: plan.estimated_before,
            estimated_tokens_after: projection.estimated_input.input_tokens,
        })
    }

    /// Emits `CompactionFailed` with the diagnostic and returns the
    /// compaction terminal for this caller.
    ///
    /// A proactive compaction failure is an actual compaction pipeline
    /// failure and settles as
    /// `AttemptFailed(Runtime(ContextCompactionFailed { message }))`; after a
    /// context overflow the original normalized overflow is preserved as the
    /// final model failure with the compaction diagnostic carried by
    /// `CompactionFailed.error`. Neither path becomes a generic context
    /// preparation failure.
    fn compaction_failure(
        &mut self,
        error: &ContextError,
        overflow: Option<&ModelError>,
    ) -> Terminal {
        self.emit(RuntimeEvent::CompactionFailed {
            error: error.message.clone(),
        });
        match overflow {
            Some(overflow) => Terminal::Failed {
                failure: AttemptFailure::Model {
                    error: overflow.clone(),
                },
            },
            None => Terminal::Failed {
                failure: AttemptFailure::Runtime {
                    error: RuntimeError::ContextCompactionFailed {
                        message: error.message.clone(),
                    },
                },
            },
        }
    }

    /// Consumes one model invocation: sends the request, assembles the
    /// provisional stream content under the given provisional identity, and
    /// returns the complete invocation (identity + assembler + terminal).
    async fn consume_invocation(
        &mut self,
        request: ModelRequest,
        provisional_message_id: &MessageId,
    ) -> Result<ModelInvocation, Terminal> {
        // The provider binding comes from the attempt's frozen snapshot: the
        // loop never resolves an adapter and never observes a later session
        // model change.
        let mut stream = self
            .request
            .model
            .primary()
            .adapter()
            .stream(request, self.cancellation.model_cancellation());
        let mut assembler = ModelEventAssembler::new();
        let terminal = match self
            .consume_model_stream(&mut assembler, provisional_message_id, &mut stream)
            .await
        {
            Ok(stream_terminal) => stream_terminal,
            Err(terminal) => return Err(terminal),
        };
        Ok(ModelInvocation {
            message_id: provisional_message_id.clone(),
            assembler,
            terminal,
        })
    }

    /// The bounded M4 compact-and-retry path after a context overflow.
    ///
    /// The compaction must retire the continuation-owning turn completely
    /// (the constraint is passed to the context engine), the pending
    /// continuation is then invalidated, and the retry request uses the
    /// smaller projection with its own deterministic retry-specific
    /// provisional/committed message identity
    /// `{attempt}-agent-{turn}-retry-{retry_number}`.
    ///
    /// The retry is a new request preparation: it composes a fresh Agent
    /// Status snapshot, and that snapshot is used for the retry's compaction
    /// hard-fit estimates and its request. The pending fresh inbound turn is
    /// deliberately not consumed by the failed overflow attempt: the retry
    /// still represents the same fresh inbound turn.
    ///
    /// The retry returns the complete retry invocation — provisional
    /// identity, assembler, and terminal together — so a successful retry
    /// replaces the failed invocation wholesale and the failed request's
    /// provisional content and tool calls are never committed or executed.
    ///
    /// If the retry also overflows, no second compaction occurs: the attempt
    /// settles with the second overflow error as its final model failure.
    async fn retry_after_overflow(
        &mut self,
        overflow_error: &ModelError,
        retry_number: u32,
    ) -> Result<ModelInvocation, Terminal> {
        if self.cancellation.is_cancelled() {
            return Err(Terminal::Cancelled {
                reason: self.cancellation.reason(),
            });
        }
        let status = match self.compose_status() {
            Ok(status) => status,
            Err(error) => return Err(Self::context_failure_terminal(&error)),
        };
        let must_cover = self.continuation_owner.clone();
        let fresh = self.pending_fresh_inbound.clone();
        match self
            .perform_compaction(
                must_cover.as_ref(),
                fresh.as_ref(),
                status.as_ref(),
                Some(overflow_error),
            )
            .await
        {
            Ok(()) => {}
            Err(terminal) => return Err(terminal),
        }
        // Cancel after completed compaction but before the retry: no model
        // retry is issued once cancellation is observable.
        if self.cancellation.is_cancelled() {
            return Err(Terminal::Cancelled {
                reason: self.cancellation.reason(),
            });
        }
        // The successful compaction already invalidated the incompatible
        // opaque provider continuation exactly once, inside
        // `perform_compaction`; this path never clears it a second time.
        self.emit(RuntimeEvent::ModelRetryScheduled {
            attempt_number: retry_number,
            retry_delay_ms: None,
        });
        let retry_message_id = MessageId::new(format!(
            "{}-agent-{}-retry-{}",
            self.request.attempt_id, self.turn, retry_number
        ));
        let request = match self.context_model_request(status.as_ref()) {
            Ok(request) => request,
            Err(terminal) => return Err(terminal),
        };
        self.emit(RuntimeEvent::ModelRequestStarted {
            model: request.model().to_owned(),
        });
        self.consume_invocation(request, &retry_message_id).await
    }

    /// Consumes one model stream: emits runtime events for non-terminal
    /// model facts, feeds the assembler, and validates the canonical stream
    /// contract. Returns the stream terminal, or the attempt terminal when
    /// the attempt must settle before the stream finished.
    async fn consume_model_stream(
        &mut self,
        assembler: &mut ModelEventAssembler,
        assistant_message_id: &MessageId,
        stream: &mut ModelEventStream,
    ) -> Result<StreamTerminal, Terminal> {
        let mut stream_terminal = None;
        while let Some(event) = stream.next().await {
            if stream_terminal.is_none() && self.cancellation.is_cancelled() {
                return Err(Terminal::Cancelled {
                    reason: self.cancellation.reason(),
                });
            }
            if let Err(error) = assembler.push(&event) {
                return Err(Terminal::Failed {
                    failure: AttemptFailure::Runtime { error },
                });
            }
            match &event {
                ModelEvent::Completed {
                    finish_reason,
                    usage,
                } => {
                    stream_terminal = Some(StreamTerminal::Completed {
                        finish_reason: finish_reason.clone(),
                        usage: usage.clone(),
                    });
                }
                ModelEvent::Failed { error } => {
                    self.emit(RuntimeEvent::ModelRequestFailed {
                        error: error.clone(),
                    });
                    stream_terminal = Some(StreamTerminal::Failed {
                        error: error.clone(),
                    });
                }
                _ => {
                    if stream_terminal.is_none() {
                        self.emit_model_event(&event, assistant_message_id);
                    }
                }
            }
        }
        let Some(stream_terminal) = stream_terminal else {
            return Err(Terminal::Failed {
                failure: AttemptFailure::Runtime {
                    error: RuntimeError::ContractViolation {
                        message: "model stream ended without a terminal event".to_owned(),
                    },
                },
            });
        };
        Ok(stream_terminal)
    }

    /// Executes the turn's tool calls through deterministic scheduling
    /// phases and commits the structurally complete result batch.
    ///
    /// Scheduling interprets [`ToolConcurrencyPolicy`] per registered tool:
    /// a `Sequential` invocation is an exclusive scheduling barrier, and
    /// adjacent `Parallel` invocations execute concurrently as one group.
    /// A background call is settled for the originating attempt when its
    /// background dispatch is accepted, not when the detached work
    /// terminates, so a sequential background call blocks later scheduling
    /// only through its dispatch-acceptance point.
    ///
    /// The structural invariant: once the valid Assistant tool-call message is
    /// committed, its entire tool-result batch is settled exactly once.
    /// Every call slot receives exactly one attempt-facing result
    /// (success/failure/cancellation/timeout/validation rejection/accepted
    /// background), canonical `ToolMessageBlock`s are committed in original
    /// model call order, and the batch never splits on cancellation. If
    /// attempt cancellation wins during the batch: in-flight cancellable
    /// foreground executions observe the attempt signal and physically
    /// settle, unstarted calls receive cancelled result slots, committed
    /// background executions stay conversation-owned, prepared-but-
    /// uncommitted dispatches roll back, and the complete batch commits in
    /// call order.
    #[allow(clippy::too_many_lines)] // one coherent scheduling/commit pipeline
    async fn execute_tools(&mut self, calls: &[ToolCall], preflight: Vec<PreflightOutcome>) {
        let mut slots: Vec<CallSlot> = calls
            .iter()
            .cloned()
            .zip(preflight)
            .map(|(call, outcome)| match outcome {
                PreflightOutcome::Ready(prepared) => CallSlot {
                    call,
                    prepared: Some(prepared),
                    result: None,
                    started: false,
                    progress: Vec::new(),
                },
                PreflightOutcome::Rejected { error } => {
                    let mut slot = CallSlot {
                        call,
                        prepared: None,
                        result: None,
                        started: false,
                        progress: Vec::new(),
                    };
                    slot.result = Some(failed_result(&error));
                    slot
                }
            })
            .collect();
        let mut index = 0;
        while index < slots.len() {
            if self.cancellation.is_cancelled() {
                break;
            }
            match group_at(&slots, index) {
                Group::Trivial => {
                    index += 1;
                }
                Group::Sequential => {
                    if slots[index].result.is_none() {
                        slots[index].started = true;
                        self.emit(RuntimeEvent::ToolExecutionStarted {
                            tool_call_id: slots[index].call.id.clone(),
                            tool_id: slots[index].tool_id(),
                        });
                        let invocation = slots[index]
                            .prepared
                            .as_ref()
                            .expect("unsettled slots are preflighted")
                            .invocation
                            .clone();
                        let (_, result, progress) = self.run_single_call(index, invocation).await;
                        slots[index].result = Some(result);
                        slots[index].progress = progress;
                    }
                    index += 1;
                }
                Group::Parallel => {
                    // Execution-start facts are emitted before any future is
                    // created, so the loop owns `&mut self` emission and the
                    // shared `&self` borrows of the spawned futures never
                    // conflict.
                    let end = parallel_group_end(&slots, index);
                    for slot in &mut slots[index..end] {
                        if slot.result.is_none() {
                            slot.started = true;
                            self.emit(RuntimeEvent::ToolExecutionStarted {
                                tool_call_id: slot.call.id.clone(),
                                tool_id: slot.tool_id(),
                            });
                        }
                    }
                    let mut futures = futures_util::stream::FuturesUnordered::new();
                    for (slot_index, slot) in slots[index..end].iter().enumerate() {
                        if slot.started {
                            let invocation = slot
                                .prepared
                                .as_ref()
                                .expect("unsettled slots are preflighted")
                                .invocation
                                .clone();
                            futures.push(Box::pin(
                                self.run_single_call(index + slot_index, invocation),
                            ));
                        }
                    }
                    let mut remaining = futures.len();
                    loop {
                        if self.cancellation.is_cancelled() {
                            break;
                        }
                        tokio::select! {
                            biased;
                            () = self.cancellation.cancelled() => break,
                            Some((slot_index, result, progress)) = futures.next() => {
                                slots[slot_index].result = Some(result);
                                slots[slot_index].progress = progress;
                                remaining -= 1;
                                if remaining == 0 {
                                    break;
                                }
                            }
                        }
                    }
                    // After cancellation wins, every in-flight foreground
                    // execution still settles: executors observe the attempt
                    // signal in their context and must settle their external
                    // work. The futures are awaited to completion, never
                    // dropped with external work abandoned.
                    while let Some((slot_index, result, progress)) = futures.next().await {
                        slots[slot_index].result = Some(result);
                        slots[slot_index].progress = progress;
                    }
                    index = end;
                }
            }
        }
        // Cancellation fill: every not-yet-started foreground call receives
        // a cancelled result slot, so the committed batch covers every
        // logical call exactly once.
        if self.cancellation.is_cancelled() {
            for slot in &mut slots {
                if slot.result.is_none() {
                    slot.result = Some(cancelled_result(self.cancellation.reason()));
                }
            }
        }
        // Canonical batch commit in original model call order. Progress
        // facts precede their completion event; the completion events
        // themselves are committed in canonical order regardless of physical
        // completion order.
        for slot in &slots {
            let result = slot.result.clone().expect("every call slot settles");
            for event in &slot.progress {
                self.emit(event.clone());
            }
            if slot.started {
                self.emit(RuntimeEvent::ToolExecutionCompleted {
                    tool_call_id: slot.call.id.clone(),
                    tool_id: slot.tool_id(),
                    result: result.clone(),
                });
            }
            let block = MessageBlock::Tool(ToolMessageBlock {
                id: MessageId::new(format!(
                    "{}-tool-{}-{}",
                    self.request.attempt_id, self.turn, slot.call.id
                )),
                tool_call_id: slot.call.id.clone(),
                tool_id: slot.tool_id(),
                result,
            });
            self.commit_canonical(&block)
                .expect("a tool result identity is unique within one attempt");
        }
        self.emit(RuntimeEvent::TurnCompleted);
    }

    /// Runs one logical tool call of the batch.
    ///
    /// Foreground invocations race against attempt cancellation (biased):
    /// when cancellation is already observable, no new execution progress
    /// begins, and an in-flight execution settles by observing the attempt
    /// signal in its context. Background invocations are dispatched through
    /// the conversation background registry's ownership commit. The slot
    /// index is returned so physical completion order can be recorded while
    /// canonical results remain model-call ordered.
    async fn run_single_call(
        &self,
        call_index: usize,
        invocation: ToolInvocation,
    ) -> (usize, ToolExecutionResult, Vec<RuntimeEvent>) {
        let (result, progress) = match invocation.mode {
            ToolInvocationMode::Foreground => self.run_foreground(&invocation).await,
            ToolInvocationMode::Background => (self.dispatch_background(&invocation), Vec::new()),
        };
        (call_index, result, progress)
    }

    /// Runs one foreground invocation against attempt cancellation.
    ///
    /// The execution receives the attempt's cancellation signal in its
    /// context, so observable attempt cancellation physically reaches
    /// cancellable native foreground work. Cancelled results produced while
    /// attempt cancellation is observable are normalized to the attempt's
    /// cancellation reason.
    async fn run_foreground(
        &self,
        invocation: &ToolInvocation,
    ) -> (ToolExecutionResult, Vec<RuntimeEvent>) {
        let executor = self.tool_registry().executor(&invocation.tool_id);
        let buffer: std::sync::Arc<std::sync::Mutex<Vec<RuntimeEvent>>> =
            std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let reporter = BufferProgressReporter {
            call_id: invocation.call_id.clone(),
            tool_id: invocation.tool_id.clone(),
            buffer: buffer.clone(),
        };
        let context = ToolExecutionContext {
            conversation_id: &self.request.conversation_id,
            execution_id: None,
            cancellation: self.cancellation.signal(),
            workspace: self.tool_runtime.workspace(),
            progress: &reporter,
            artifacts: self.tool_runtime.artifacts(),
            environment: self.capability.snapshot().effective_environment(),
        };
        let future = executor.execute(invocation.clone(), context);
        tokio::pin!(future);
        let mut result = tokio::select! {
            biased;
            () = self.cancellation.cancelled() => future.as_mut().await,
            result = future.as_mut() => result,
        };
        if self.cancellation.is_cancelled()
            && matches!(result.status, ToolExecutionStatus::Cancelled { .. })
        {
            result.status = ToolExecutionStatus::Cancelled {
                reason: self.cancellation.reason(),
            };
        }
        let progress_events = std::mem::take(&mut *buffer.lock().expect("progress buffer lock"));
        (result, progress_events)
    }

    /// Dispatches one background invocation through the conversation-owned
    /// background registry.
    ///
    /// The dispatch is two-stage: prepare allocates the deterministic
    /// execution id and parks the runner behind its commit gate; commit
    /// performs the final attempt-cancellation checkpoint and produces the
    /// accepted result exactly when conversation ownership commits. A
    /// rolled-back dispatch produces a cancelled result slot for the
    /// originating attempt, never a detached execution.
    fn dispatch_background(&self, invocation: &ToolInvocation) -> ToolExecutionResult {
        let executor = self.tool_registry().executor(&invocation.tool_id);
        // The attempt's effective ToolEnvironment is captured at prepare
        // time, before the background ownership commit: the detached
        // execution retains exactly this immutable environment for its
        // whole lifetime, even after this attempt terminates and later
        // revisions activate.
        let environment = self.capability.snapshot().effective_environment().clone();
        match self
            .tool_runtime
            .background()
            .prepare_dispatch(invocation, &executor, environment)
        {
            Ok(prepared) => {
                match self
                    .tool_runtime
                    .background()
                    .commit_dispatch(prepared, &self.cancellation.signal())
                {
                    BackgroundDispatchOutcome::Accepted { result, .. } => result,
                    BackgroundDispatchOutcome::RolledBack => {
                        cancelled_result(self.cancellation.reason())
                    }
                }
            }
            Err(error) => failed_result(&error.to_string()),
        }
    }

    /// Builds the canonical request from one finite context projection.
    ///
    /// Every projected item is already a complete canonical message, so
    /// there is nothing to materialize: the projection's messages *are* the
    /// request messages. The ephemeral Agent Status and Skill catalog
    /// attachments travel alongside; neither is ever encoded as a fake
    /// canonical message.
    fn model_request_from_projection(&self, projection: &ContextProjection) -> ModelRequest {
        let primary = self.request.model.primary();
        // Tool definitions are compiled only for a model whose effective
        // capabilities include tool calls: a text-only model is usable, it
        // simply never receives runtime tool definitions.
        let tools = if primary.capabilities().tool_calls {
            self.tool_registry().model_definitions()
        } else {
            Vec::new()
        };
        ModelRequest {
            invocation: primary.invocation_config(),
            messages: projection.messages.clone(),
            tools,
            agent_status: projection.agent_status.clone(),
            skill_catalog: projection.skill_catalog.clone(),
            continuation: self.pending_continuation.clone(),
        }
    }

    /// Commits the assembled Assistant message into the conversation state.
    fn commit_assistant_message(
        &mut self,
        message_id: &MessageId,
        content: &[crate::message::types::AssistantContentBlock],
    ) -> Result<(), ConversationError> {
        self.commit_canonical(&MessageBlock::Assistant(AssistantMessageBlock {
            id: message_id.clone(),
            content: content.to_vec(),
        }))?;
        Ok(())
    }

    /// The one canonical commit path of the loop: one Message Ledger append
    /// plus one Conversation Surface append, followed by the commit
    /// observation at that same linearization point.
    ///
    /// Every canonical commit of the attempt — drained inbound user
    /// messages, committed Assistant messages, committed Tool messages — goes
    /// through here. Independent ledger/surface mutations do not exist in
    /// this module.
    fn commit_canonical(&mut self, block: &MessageBlock) -> Result<MessageId, ConversationError> {
        let message_id = self.conversation.commit(block.clone())?;
        if let Some(observer) = self.observer {
            observer.observe_committed(&self.request.attempt_id, block);
        }
        Ok(message_id)
    }

    /// Emits the runtime events for one non-terminal model event.
    fn emit_model_event(&mut self, event: &ModelEvent, message_id: &MessageId) {
        match event {
            ModelEvent::Started => {
                self.emit(RuntimeEvent::AssistantMessageStarted {
                    message_id: message_id.clone(),
                });
            }
            ModelEvent::TextDelta { block_index, text } => {
                self.emit(RuntimeEvent::AssistantTextDelta {
                    message_id: message_id.clone(),
                    block_index: *block_index,
                    delta: text.clone(),
                });
            }
            ModelEvent::ReasoningDelta { block_index, text } => {
                self.emit(RuntimeEvent::AssistantReasoningDelta {
                    message_id: message_id.clone(),
                    block_index: *block_index,
                    delta: text.clone(),
                });
            }
            ModelEvent::RefusalDelta { block_index, text } => {
                self.emit(RuntimeEvent::AssistantRefusalDelta {
                    message_id: message_id.clone(),
                    block_index: *block_index,
                    delta: text.clone(),
                });
            }
            ModelEvent::ToolCallStarted { block_index, call } => {
                self.emit(RuntimeEvent::ToolCallStarted {
                    message_id: message_id.clone(),
                    block_index: *block_index,
                    call: call.clone(),
                });
            }
            ModelEvent::ToolCallArgumentsDelta {
                block_index,
                call_id,
                arguments_delta,
            } => {
                self.emit(RuntimeEvent::ToolCallArgumentsDelta {
                    message_id: message_id.clone(),
                    block_index: *block_index,
                    call_id: call_id.clone(),
                    arguments_delta: arguments_delta.clone(),
                });
            }
            ModelEvent::ToolCallCompleted { block_index, call } => {
                self.emit(RuntimeEvent::ToolCallCompleted {
                    message_id: message_id.clone(),
                    block_index: *block_index,
                    call: call.clone(),
                });
            }
            ModelEvent::UsageUpdate { .. } | ModelEvent::ContinuationState { .. } => {}
            ModelEvent::Completed { .. } | ModelEvent::Failed { .. } => unreachable!(),
        }
    }

    fn emit(&mut self, event: RuntimeEvent) {
        debug_assert!(
            !self.terminal_emitted,
            "no runtime events may follow the terminal event"
        );
        if let Some(observer) = self.observer {
            observer.observe_event(&self.request.attempt_id, &event);
        }
        self.events.push(event);
    }

    fn emit_terminal(&mut self, terminal: &Terminal) {
        let event = match terminal {
            Terminal::Completed { finish_reason } => RuntimeEvent::AttemptCompleted {
                attempt_id: self.request.attempt_id.clone(),
                finish_reason: finish_reason.clone(),
            },
            Terminal::Cancelled { reason } => RuntimeEvent::AttemptCancelled {
                attempt_id: self.request.attempt_id.clone(),
                reason: *reason,
            },
            Terminal::Failed { failure } => RuntimeEvent::AttemptFailed {
                attempt_id: self.request.attempt_id.clone(),
                error: failure.clone(),
            },
        };
        debug_assert!(!self.terminal_emitted, "exactly one terminal event");
        self.terminal_emitted = true;
        if let Some(observer) = self.observer {
            observer.observe_event(&self.request.attempt_id, &event);
        }
        self.events.push(event);
    }
}

/// One result slot of a committed tool-call batch, preallocated in model
/// call order so completion timing can never influence message identities or
/// canonical ordering.
struct CallSlot {
    call: ToolCall,
    prepared: Option<PreparedInvocation>,
    result: Option<ToolExecutionResult>,
    started: bool,
    progress: Vec<RuntimeEvent>,
}

impl CallSlot {
    fn tool_id(&self) -> ToolId {
        match &self.prepared {
            Some(prepared) => prepared.invocation.tool_id.clone(),
            None => self.call.tool_id.clone(),
        }
    }
}

/// One deterministic scheduling phase of a tool-call batch.
enum Group {
    /// The slot is already settled (validation rejection); no barrier.
    Trivial,
    /// An exclusive scheduling barrier.
    Sequential,
    /// Adjacent parallel invocations executing concurrently.
    Parallel,
}

/// The scheduling phase beginning at `index`: a `Sequential` invocation is
/// an exclusive barrier; adjacent `Parallel` invocations form one group.
fn group_at(slots: &[CallSlot], index: usize) -> Group {
    let Some(prepared) = slots[index].prepared.as_ref() else {
        return Group::Trivial;
    };
    match prepared.concurrency {
        ToolConcurrencyPolicy::Sequential => Group::Sequential,
        ToolConcurrencyPolicy::Parallel => Group::Parallel,
    }
}

/// The exclusive end of the parallel group beginning at `index`: every
/// adjacent `Parallel` invocation forms one group.
fn parallel_group_end(slots: &[CallSlot], index: usize) -> usize {
    let mut end = index + 1;
    while end < slots.len()
        && slots[end]
            .prepared
            .as_ref()
            .is_some_and(|candidate| candidate.concurrency == ToolConcurrencyPolicy::Parallel)
    {
        end += 1;
    }
    end
}

/// A failed tool result.
fn failed_result(error: &str) -> ToolExecutionResult {
    ToolExecutionResult {
        status: ToolExecutionStatus::Failed {
            error: error.to_owned(),
        },
        content: Vec::new(),
        duration_ms: 0,
        exit_code: None,
        artifacts: Vec::new(),
        truncation: None,
    }
}

/// A cancelled tool result carrying the attempt cancellation reason.
fn cancelled_result(reason: CancellationReason) -> ToolExecutionResult {
    ToolExecutionResult {
        status: ToolExecutionStatus::Cancelled { reason },
        content: Vec::new(),
        duration_ms: 0,
        exit_code: None,
        artifacts: Vec::new(),
        truncation: None,
    }
}

/// The foreground progress reporter of one execution: progress facts are
/// normalized through the one shared UTF-8-safe bound
/// ([`bound_tool_progress`], the same normalization the background registry
/// uses), buffered per slot, and become canonical `ToolExecutionProgress`
/// events at batch commit, before their completion event.
struct BufferProgressReporter {
    call_id: ToolCallId,
    tool_id: ToolId,
    buffer: std::sync::Arc<std::sync::Mutex<Vec<RuntimeEvent>>>,
}

impl ProgressReporter for BufferProgressReporter {
    fn report(&self, progress: ToolProgress) {
        let bounded = crate::tools::limits::bound_tool_progress(progress);
        self.buffer.lock().expect("progress buffer lock").push(
            RuntimeEvent::ToolExecutionProgress {
                tool_call_id: self.call_id.clone(),
                tool_id: self.tool_id.clone(),
                execution_id: None,
                progress: bounded,
            },
        );
    }
}

/// Test-only synchronization for in-crate unit tests.
///
/// [`ContinuationBoundaryPause`] parks the execution at the turn-continuation
/// boundary — after a completed turn (including every mailbox drain/append
/// of that turn) returned "continue", before the generic
/// cancellation-before-next-turn check — so a unit test can make
/// cancellation observable deterministically between turns, without timing
/// assumptions.
///
/// The pause signals `reached` through a watch (observed with `wait_for`)
/// and blocks the execution task on a `std` channel until the test
/// releases it, so the controlling test must run on a multi-threaded
/// runtime. This hook exists only under `#[cfg(test)]`.
#[cfg(test)]
mod test_sync {
    use std::sync::mpsc;

    use tokio::sync::watch;

    /// A test-only control point at the turn-continuation boundary.
    ///
    /// The execution parks here exactly when a completed turn returned
    /// "continue" — the turn is structurally complete and every mailbox
    /// drain/append of that turn is done — before the generic
    /// cancellation-before-next-turn check runs. A unit test can therefore
    /// make cancellation observable deterministically after one turn
    /// completed but before another starts, without timing assumptions.
    ///
    /// The pause signals `reached` through a watch (observed with
    /// `wait_for`) and blocks the execution task on a `std` channel until
    /// the test releases it, so the controlling test must run on a
    /// multi-threaded runtime. This hook exists only under `#[cfg(test)]`.
    #[derive(Debug)]
    pub(super) struct ContinuationBoundaryPause {
        reached: watch::Sender<bool>,
        release: mpsc::Receiver<()>,
    }

    impl ContinuationBoundaryPause {
        /// Creates the pause and its observation/release handles.
        #[must_use]
        pub(super) fn install() -> (Self, watch::Receiver<bool>, mpsc::Sender<()>) {
            let (reached, reached_rx) = watch::channel(false);
            let (release_tx, release_rx) = mpsc::channel();
            (
                Self {
                    reached,
                    release: release_rx,
                },
                reached_rx,
                release_tx,
            )
        }

        /// Signals that the turn boundary was reached, then blocks until
        /// the test releases the execution.
        pub(super) fn park_at_continuation_boundary(&self) {
            self.reached.send_replace(true);
            let _ = self.release.recv();
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::conversation::ConversationState;
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    use futures_util::future::BoxFuture;

    use crate::message::types::{
        ContentBlockIndex, MessageBlock, UserContentBlock, UserMessageBlock, UserSource,
    };
    use crate::model::adapter::{ModelAdapter, ModelEventStream};
    use crate::model::chat_protocol;
    use crate::model::event::ModelEvent;
    use crate::model::finish::ModelFinishReason;
    use crate::model::types::{ModelProtocol, ModelRequest};
    use crate::runtime::cancellation::CancellationSignal;
    use crate::runtime::identity::{
        AgentId, AttemptId, ConversationId, MessageId, ToolCallId, ToolId,
    };
    use crate::runtime::inbound::{ConversationInboundMailbox, InitialTurnTrigger};
    use crate::runtime::types::CancellationReason;
    use crate::scripted_suites::support::model::scripted_session_model;
    use crate::tools::executor::{ToolExecutor, ToolRegistry};
    use crate::tools::types::{
        ToolCall, ToolCallStart, ToolConcurrencyPolicy, ToolDefinition, ToolExecutionPolicy,
        ToolExecutionResult, ToolExecutionStatus, ToolInvocation, ToolOrigin, ToolReplayPolicy,
    };

    use super::{AgentExecution, AgentExecutionRequest, test_sync::ContinuationBoundaryPause};
    use crate::agent::cancellation::AgentCancellation;
    use crate::context::ContextRuntime;

    /// A scripted model adapter: each invocation pops the next event script
    /// and yields it synchronously, recording every request.
    struct ScriptedAdapter {
        scripts: Mutex<VecDeque<Vec<ModelEvent>>>,
        requests: Arc<Mutex<Vec<ModelRequest>>>,
    }

    impl ScriptedAdapter {
        fn new(scripts: Vec<Vec<ModelEvent>>) -> Self {
            Self {
                scripts: Mutex::new(scripts.into()),
                requests: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn request_count(&self) -> usize {
            self.requests
                .lock()
                .expect("scripted adapter request lock")
                .len()
        }
    }

    impl ModelAdapter for ScriptedAdapter {
        fn protocol(&self) -> ModelProtocol {
            chat_protocol()
        }

        fn stream(
            &self,
            request: ModelRequest,
            _cancellation: CancellationSignal,
        ) -> ModelEventStream {
            self.requests
                .lock()
                .expect("scripted adapter request lock")
                .push(request);
            let script = self
                .scripts
                .lock()
                .expect("scripted adapter script lock")
                .pop_front()
                .unwrap_or_default();
            Box::pin(futures_util::stream::iter(script))
        }
    }

    /// An instant fake executor returning one fixed successful result.
    struct InstantTool;

    impl InstantTool {
        fn definition(id: &str, name: &str) -> ToolDefinition {
            ToolDefinition {
                id: ToolId::new(id),
                name: name.to_owned(),
                description: String::new(),
                input_schema: serde_json::json!({"type": "object"}),
                execution_policy: ToolExecutionPolicy::ForegroundOnly,
                concurrency_policy: ToolConcurrencyPolicy::Sequential,
                replay_policy: ToolReplayPolicy::Never,
                origin: ToolOrigin::Builtin,
            }
        }
    }

    impl ToolExecutor for InstantTool {
        fn execute<'a>(
            &'a self,
            _invocation: ToolInvocation,
            _context: crate::tools::executor::ToolExecutionContext<'a>,
        ) -> BoxFuture<'a, ToolExecutionResult> {
            Box::pin(async {
                ToolExecutionResult {
                    status: ToolExecutionStatus::Success,
                    content: Vec::new(),
                    duration_ms: 0,
                    exit_code: None,
                    artifacts: Vec::new(),
                    truncation: None,
                }
            })
        }
    }

    /// One attempt request bound to a scripted adapter through a real
    /// catalog resolution: the binding still requires an explicit endpoint
    /// and credential, exactly as in production.
    fn request(adapter: &Arc<ScriptedAdapter>) -> AgentExecutionRequest {
        let adapter: Arc<dyn ModelAdapter> = adapter.clone();
        AgentExecutionRequest {
            agent_id: AgentId::new("agent-a"),
            conversation_id: ConversationId::new("conv-1"),
            attempt_id: AttemptId::new("attempt-1"),
            conversation: ConversationState::new(),
            initial_turn_trigger: InitialTurnTrigger::Continuation,
            timezone: None,
            model: scripted_session_model(adapter).snapshot(),
        }
    }

    /// A deterministic context runtime derived from the same immutable model
    /// snapshot as the execution. The window is far larger than any scripted
    /// request, so no compaction ever triggers in these tests.
    /// A conversation tool runtime over a temporary workspace.
    fn tool_runtime() -> crate::tools::runtime::ConversationToolRuntime {
        tool_runtime_with_mailbox(None)
    }

    /// A conversation tool runtime over a temporary workspace with an
    /// optional explicitly configured conversation mailbox.
    fn tool_runtime_with_mailbox(
        mailbox: Option<ConversationInboundMailbox>,
    ) -> crate::tools::runtime::ConversationToolRuntime {
        use crate::tools::runtime::ConversationRuntimeConfig;
        let dir = std::env::temp_dir().join(format!(
            "rustx-agent-crate-tests-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("worker")
        ));
        let _ = std::fs::create_dir_all(dir.join("workspace"));
        crate::tools::runtime::ConversationToolRuntime::from_config(
            ConversationId::new("conv-1"),
            ConversationRuntimeConfig {
                mailbox,
                ..ConversationRuntimeConfig::new(dir.join("workspace"), dir.join("artifacts"))
            },
        )
        .expect("tool runtime")
    }

    fn runtime(adapter: &Arc<ScriptedAdapter>) -> ContextRuntime {
        ContextRuntime::for_attempt(
            crate::context::SessionContextPolicy {
                reserve_tokens: 0,
                keep_recent_tokens: 0,
                summary_output_cap: None,
            },
            Arc::new(crate::context::DefaultTokenEstimator),
            crate::context::AgentStatusComposer::default(),
            &request(adapter).model,
        )
        .expect("valid context runtime")
    }

    fn inbound_message(id: &str, text: &str) -> UserMessageBlock {
        UserMessageBlock {
            id: MessageId::new(id),
            content: vec![UserContentBlock::Text(crate::message::content::TextBlock {
                text: text.to_owned(),
            })],
            source: UserSource::Human,
            kind: crate::message::types::InboundKind::Message,
            timestamp: Some(
                chrono::DateTime::parse_from_rfc3339("2026-08-07T12:00:00Z")
                    .expect("parse fixed timestamp")
                    .with_timezone(&chrono::Utc),
            ),
        }
    }

    /// One turn of a single tool call, scripted as canonical events.
    fn tool_call_script(call: &ToolCall) -> Vec<ModelEvent> {
        vec![
            ModelEvent::Started,
            ModelEvent::ToolCallStarted {
                block_index: ContentBlockIndex::new(0),
                call: ToolCallStart {
                    id: call.id.clone(),
                    tool_id: call.tool_id.clone(),
                    name: call.name.clone(),
                },
            },
            ModelEvent::ToolCallArgumentsDelta {
                block_index: ContentBlockIndex::new(0),
                call_id: call.id.clone(),
                arguments_delta: "{}".to_owned(),
            },
            ModelEvent::ToolCallCompleted {
                block_index: ContentBlockIndex::new(0),
                call: call.clone(),
            },
            ModelEvent::Completed {
                finish_reason: ModelFinishReason::ToolCalls,
                usage: None,
            },
        ]
    }

    /// The exact expected trace: one completed tool turn, then the generic
    /// pre-next-turn cancellation checkpoint settles the attempt cancelled
    /// before any second model turn.
    fn expected_trace() -> Vec<crate::events::types::RuntimeEvent> {
        use crate::events::types::RuntimeEvent;
        vec![
            RuntimeEvent::AttemptStarted {
                attempt_id: AttemptId::new("attempt-1"),
            },
            RuntimeEvent::TurnStarted,
            RuntimeEvent::ModelRequestStarted {
                model: "scripted".to_owned(),
            },
            RuntimeEvent::AssistantMessageStarted {
                message_id: MessageId::new("attempt-1-agent-1"),
            },
            RuntimeEvent::ToolCallStarted {
                message_id: MessageId::new("attempt-1-agent-1"),
                block_index: ContentBlockIndex::new(0),
                call: ToolCallStart {
                    id: ToolCallId::new("call-1"),
                    tool_id: ToolId::new("tool-alpha"),
                    name: "alpha".to_owned(),
                },
            },
            RuntimeEvent::ToolCallArgumentsDelta {
                message_id: MessageId::new("attempt-1-agent-1"),
                block_index: ContentBlockIndex::new(0),
                call_id: ToolCallId::new("call-1"),
                arguments_delta: "{}".to_owned(),
            },
            RuntimeEvent::ToolCallCompleted {
                message_id: MessageId::new("attempt-1-agent-1"),
                block_index: ContentBlockIndex::new(0),
                call: ToolCall {
                    id: ToolCallId::new("call-1"),
                    tool_id: ToolId::new("tool-alpha"),
                    name: "alpha".to_owned(),
                    arguments: serde_json::json!({}),
                },
            },
            RuntimeEvent::ModelRequestCompleted {
                finish_reason: ModelFinishReason::ToolCalls,
                usage: None,
            },
            RuntimeEvent::ToolExecutionStarted {
                tool_call_id: ToolCallId::new("call-1"),
                tool_id: ToolId::new("tool-alpha"),
            },
            RuntimeEvent::ToolExecutionCompleted {
                tool_call_id: ToolCallId::new("call-1"),
                tool_id: ToolId::new("tool-alpha"),
                result: ToolExecutionResult {
                    status: ToolExecutionStatus::Success,
                    content: Vec::new(),
                    duration_ms: 0,
                    exit_code: None,
                    artifacts: Vec::new(),
                    truncation: None,
                },
            },
            RuntimeEvent::TurnCompleted,
            RuntimeEvent::AttemptCancelled {
                attempt_id: AttemptId::new("attempt-1"),
                reason: CancellationReason::UserRequested,
            },
        ]
    }

    /// Spawns the controller that parks until the continuation boundary,
    /// makes cancellation observable there, and releases the execution.
    fn boundary_controller(
        mut reached_rx: tokio::sync::watch::Receiver<bool>,
        release_tx: std::sync::mpsc::Sender<()>,
        cancellation: AgentCancellation,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            reached_rx
                .wait_for(|reached| *reached)
                .await
                .expect("continuation boundary reached");
            cancellation.cancel();
            release_tx.send(()).expect("release the execution");
        })
    }

    /// Builds the attempt capability lease over the given tool registry and
    /// conversation tool runtime: empty Skill set, base environment, and a
    /// private environment store. Returns the store guard, the coordinator,
    /// and the pinned lease.
    async fn capability_lease(
        tools: ToolRegistry,
        tool_runtime: &crate::tools::runtime::ConversationToolRuntime,
    ) -> (
        tempfile::TempDir,
        crate::capabilities::CapabilityCoordinator,
        crate::capabilities::AttemptCapabilityLease,
    ) {
        let tools = std::sync::Arc::new(tools);
        let dir = tempfile::tempdir().expect("temp dir");
        let coordinator = crate::capabilities::CapabilityCoordinator::new(
            crate::capabilities::CapabilityCoordinatorConfig {
                conversation_id: tool_runtime.conversation_id().clone(),
                workspace: tool_runtime.workspace().clone(),
                base_tool_registry: tools,
                mcp_servers: std::collections::BTreeMap::new(),
                base_environment: tool_runtime.environment().clone(),
                environment_store_root: dir.path().join("env-store"),
            },
        )
        .expect("capability coordinator");
        let candidate = coordinator.prepare_candidate().await.expect("prepare");
        coordinator.commit(candidate).expect("commit");
        let lease = coordinator.acquire_attempt_lease();
        (dir, coordinator, lease)
    }

    #[tokio::test]
    async fn capability_lease_owner_matches_runtime_before_execution() {
        let adapter = Arc::new(ScriptedAdapter::new(Vec::new()));
        let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
        let tool_runtime = tool_runtime();
        let (_dir, coordinator, lease) = capability_lease(ToolRegistry::new(), &tool_runtime).await;
        assert_eq!(coordinator.active_attempts(), 1);

        let execution = AgentExecution::new(
            request(&adapter),
            lease,
            &cancellation,
            runtime(&adapter),
            &tool_runtime,
        )
        .expect("matching capability owner is accepted");

        assert_eq!(adapter.request_count(), 0, "construction is pre-execution");
        drop(execution);
        assert_eq!(coordinator.active_attempts(), 0);
    }

    #[tokio::test]
    async fn capability_lease_rejects_different_conversation_before_execution() {
        let adapter = Arc::new(ScriptedAdapter::new(Vec::new()));
        let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
        let owner_runtime = tool_runtime();
        let (_dir, coordinator, lease) =
            capability_lease(ToolRegistry::new(), &owner_runtime).await;
        assert_eq!(coordinator.active_attempts(), 1);
        let other_dir = tempfile::tempdir().expect("other runtime directory");
        std::fs::create_dir_all(other_dir.path().join("workspace")).expect("other workspace");
        let other_runtime = crate::tools::runtime::ConversationToolRuntime::new(
            ConversationId::new("conv-2"),
            other_dir.path().join("workspace"),
            other_dir.path().join("artifacts"),
        )
        .expect("other tool runtime");
        let mut other_request = request(&adapter);
        other_request.conversation_id = ConversationId::new("conv-2");

        let result = AgentExecution::new(
            other_request,
            lease,
            &cancellation,
            runtime(&adapter),
            &other_runtime,
        );

        assert!(matches!(
            result,
            Err(crate::runtime::inbound::MailboxError::CapabilityOwnershipMismatch { .. })
        ));
        assert_eq!(
            adapter.request_count(),
            0,
            "rejection precedes model requests"
        );
        assert_eq!(coordinator.active_attempts(), 0);
    }

    #[tokio::test]
    async fn capability_lease_rejects_different_workspace_before_execution() {
        let adapter = Arc::new(ScriptedAdapter::new(Vec::new()));
        let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
        let owner_runtime = tool_runtime();
        let (_dir, coordinator, lease) =
            capability_lease(ToolRegistry::new(), &owner_runtime).await;
        assert_eq!(coordinator.active_attempts(), 1);
        let other_dir = tempfile::tempdir().expect("other workspace directory");
        std::fs::create_dir_all(other_dir.path().join("workspace")).expect("other workspace");
        let other_runtime = crate::tools::runtime::ConversationToolRuntime::new(
            ConversationId::new("conv-1"),
            other_dir.path().join("workspace"),
            other_dir.path().join("artifacts"),
        )
        .expect("other tool runtime");

        let result = AgentExecution::new(
            request(&adapter),
            lease,
            &cancellation,
            runtime(&adapter),
            &other_runtime,
        );

        assert!(matches!(
            result,
            Err(crate::runtime::inbound::MailboxError::CapabilityOwnershipMismatch { .. })
        ));
        assert_eq!(
            adapter.request_count(),
            0,
            "rejection precedes model requests"
        );
        assert_eq!(coordinator.active_attempts(), 0);
    }

    /// The generic turn-boundary invariant with no mailbox attached: turn 1
    /// completes with a tool call and its result, the test control point
    /// makes cancellation observable after the turn (and all of its work)
    /// completed but before the next turn begins, and the generic
    /// pre-next-turn checkpoint settles the attempt cancelled — the second
    /// model turn never starts.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancellation_at_turn_boundary_stops_next_model_request_without_mailbox() {
        let call = ToolCall {
            id: ToolCallId::new("call-1"),
            tool_id: ToolId::new("tool-alpha"),
            name: "alpha".to_owned(),
            arguments: serde_json::json!({}),
        };
        let adapter = Arc::new(ScriptedAdapter::new(vec![tool_call_script(&call)]));
        let mut tools = ToolRegistry::new();
        tools
            .register(
                InstantTool::definition("tool-alpha", "alpha"),
                std::sync::Arc::new(InstantTool),
            )
            .expect("register tool");
        let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
        let (pause, reached_rx, release_tx) = ContinuationBoundaryPause::install();
        let controller = boundary_controller(reached_rx, release_tx, cancellation.clone());

        let tool_runtime = tool_runtime();
        let (_dir, _coordinator, lease) = capability_lease(tools, &tool_runtime).await;
        let execution = AgentExecution::new(
            request(&adapter),
            lease,
            &cancellation,
            runtime(&adapter),
            &tool_runtime,
        )
        .expect("conversation identity matches the tool runtime");
        execution
            .continuation_pause
            .lock()
            .expect("continuation pause lock")
            .replace(pause);
        let result = execution.run().await;
        controller.await.expect("controller task");

        assert_eq!(
            adapter.request_count(),
            1,
            "exactly one model request total: the second model turn never begins"
        );
        assert_eq!(
            result
                .events
                .iter()
                .filter(|event| matches!(event, crate::events::types::RuntimeEvent::TurnStarted))
                .count(),
            1,
            "exactly one TurnStarted total"
        );
        assert_eq!(
            result
                .events
                .iter()
                .filter(|event| {
                    matches!(
                        event,
                        crate::events::types::RuntimeEvent::ModelRequestStarted { .. }
                    )
                })
                .count(),
            1,
            "exactly one ModelRequestStarted total"
        );
        assert_eq!(
            result.events,
            expected_trace(),
            "the exact trace ends with the single AttemptCancelled terminal event"
        );
        assert_eq!(
            result.outcome,
            crate::events::types::AttemptOutcome::Cancelled {
                reason: CancellationReason::UserRequested,
            }
        );
    }

    /// The drain+append commit-point interleaving on the generic boundary
    /// hook: a tool turn completes, the safe boundary atomically drains
    /// batch A and appends it to canonical history, the continuation
    /// boundary control point makes cancellation observable there, and
    /// after the release the generic pre-next-turn checkpoint prevents any
    /// second model turn — mailbox commit semantics and generic Agent Loop
    /// cancellation compose.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancellation_after_drain_append_stops_before_the_next_turn() {
        let call = ToolCall {
            id: ToolCallId::new("call-1"),
            tool_id: ToolId::new("tool-alpha"),
            name: "alpha".to_owned(),
            arguments: serde_json::json!({}),
        };
        let adapter = Arc::new(ScriptedAdapter::new(vec![tool_call_script(&call)]));
        let mut tools = ToolRegistry::new();
        tools
            .register(
                InstantTool::definition("tool-alpha", "alpha"),
                std::sync::Arc::new(InstantTool),
            )
            .expect("register tool");
        let mailbox = ConversationInboundMailbox::new(ConversationId::new("conv-1"));
        mailbox
            .enqueue(inbound_message("msg-a", "A"))
            .expect("enqueue A before the attempt");
        let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
        let (pause, reached_rx, release_tx) = ContinuationBoundaryPause::install();
        let controller = boundary_controller(reached_rx, release_tx, cancellation.clone());

        let tool_runtime = tool_runtime_with_mailbox(Some(mailbox.clone()));
        let (_dir, _coordinator, lease) = capability_lease(tools, &tool_runtime).await;
        let execution = AgentExecution::new(
            request(&adapter),
            lease,
            &cancellation,
            runtime(&adapter),
            &tool_runtime,
        )
        .expect("conversation identity matches the tool runtime");
        execution
            .continuation_pause
            .lock()
            .expect("continuation pause lock")
            .replace(pause);
        let result = execution.run().await;
        controller.await.expect("controller task");

        assert_eq!(
            adapter.request_count(),
            1,
            "no next model turn begins after the drained batch is committed"
        );
        assert_eq!(
            result.events,
            expected_trace(),
            "the exact trace ends with the single AttemptCancelled terminal event"
        );
        assert_eq!(
            result.outcome,
            crate::events::types::AttemptOutcome::Cancelled {
                reason: CancellationReason::UserRequested,
            }
        );
        let committed: Vec<&MessageBlock> = result
            .messages()
            .iter()
            .filter(|block| {
                matches!(block, MessageBlock::User(user) if user.id == MessageId::new("msg-a"))
            })
            .collect();
        assert_eq!(
            committed.len(),
            1,
            "the drained batch appears exactly once in canonical history"
        );
        assert!(
            mailbox.drain().is_none(),
            "the appended batch is consumed from the mailbox and never requeued"
        );
    }

    /// A parking background executor: starts, waits for the release notify,
    /// and then settles with a fixed result.
    struct ParkingBackgroundTool {
        definition: ToolDefinition,
        started: tokio::sync::watch::Sender<bool>,
        release: Arc<tokio::sync::Notify>,
    }

    impl ParkingBackgroundTool {
        fn new() -> (
            Self,
            tokio::sync::watch::Receiver<bool>,
            Arc<tokio::sync::Notify>,
        ) {
            let (started, started_rx) = tokio::sync::watch::channel(false);
            let release = Arc::new(tokio::sync::Notify::new());
            (
                Self {
                    definition: ToolDefinition {
                        id: ToolId::new("tool-bg"),
                        name: "bg".to_owned(),
                        description: String::new(),
                        input_schema: serde_json::json!({"type": "object"}),
                        execution_policy: ToolExecutionPolicy::ModelSelectable,
                        concurrency_policy: ToolConcurrencyPolicy::Sequential,
                        replay_policy: ToolReplayPolicy::Never,
                        origin: ToolOrigin::Builtin,
                    },
                    started,
                    release: release.clone(),
                },
                started_rx,
                release,
            )
        }
    }

    impl ToolExecutor for ParkingBackgroundTool {
        fn execute<'a>(
            &'a self,
            _invocation: ToolInvocation,
            _context: crate::tools::executor::ToolExecutionContext<'a>,
        ) -> BoxFuture<'a, ToolExecutionResult> {
            self.started.send_replace(true);
            let release = self.release.clone();
            Box::pin(async move {
                release.notified().await;
                ToolExecutionResult {
                    status: ToolExecutionStatus::Success,
                    content: Vec::new(),
                    duration_ms: 0,
                    exit_code: None,
                    artifacts: Vec::new(),
                    truncation: None,
                }
            })
        }
    }

    /// The outer liveness guard of the deterministic mailbox-boundary test.
    ///
    /// Nothing in that test's ordering proof depends on this value: every
    /// semantic step is an exact handshake. It bounds only total wall time
    /// so a genuine regression fails with a message instead of hanging a
    /// CI job, and it is deliberately far larger than any scheduling delay
    /// a loaded runner can produce.
    const LIVENESS_GUARD: std::time::Duration = std::time::Duration::from_secs(120);

    /// Receives one mailbox-probe token without occupying a Tokio worker
    /// thread, returning the receiver for the next step.
    ///
    /// The wait is exact: no bound participates in the ordering proof. The
    /// only bounded waits in this test are the outer liveness guards around
    /// the attempt and the controller.
    async fn recv_probe_token(
        receiver: std::sync::mpsc::Receiver<()>,
        what: &'static str,
    ) -> std::sync::mpsc::Receiver<()> {
        tokio::task::spawn_blocking(move || {
            receiver.recv().unwrap_or_else(|error| {
                panic!("{what}: the probe channel closed before the token arrived ({error})")
            });
            receiver
        })
        .await
        .expect("probe receive task")
    }

    /// Exact mailbox-boundary proof for the background terminal inbound.
    ///
    /// The production finite-snapshot contract under test:
    ///
    /// ```text
    /// once a safe-boundary drain committed its finite snapshot under the
    /// mailbox mutex (watermark selected, items detached), an inbound whose
    /// enqueue linearizes after that snapshot can never join that batch
    /// ```
    ///
    /// The test constructs the exact happens-before chain deterministically;
    /// "post-first-snapshot" alone does not imply "pre-second-snapshot", so
    /// both sides of the ordering are proven explicitly:
    ///
    /// ```text
    /// [human] enqueued and published before the attempt starts
    /// turn 1 dispatches the parking background tool
    /// safe-boundary drain #1 commits snapshot [human] (observed through
    ///   drain_snapshot while the drain still holds the mailbox mutex) —
    ///   the first finite batch is fixed forever at this point
    /// background runner started → released → its terminal enqueue can only
    ///   block on the mailbox mutex drain #1 still owns
    /// drain #1 released → batch [human] appended, turn 1 completes
    /// Agent Loop parks at the test-only continuation boundary before
    ///   turn 2 — no further drain can occur while it is parked
    /// terminal enqueue owns the mailbox mutex (enqueue_computed observed):
    ///   provably after drain #1 released it and before any later drain
    /// terminal published (enqueue_resume → pending.push_back)
    /// Agent Loop released → turn 2 → request #2 ([human], no terminal)
    /// safe-boundary drain #2 commits snapshot [terminal]
    /// turn 3 → request #3 observes the terminal
    /// ```
    ///
    /// `enqueue_computed` is not the publication linearization point: the
    /// item becomes pending only at `pending.push_back` under the same
    /// mutex. What `enqueue_computed` proves is mutex ownership — the
    /// terminal enqueue acquired the same mailbox mutex after drain #1
    /// released it, so no later drain can overtake it while it is parked.
    /// No sleep or scheduler-timing assumption participates in the proof.
    ///
    /// # Why the controller receives through the blocking pool
    ///
    /// The mailbox probe hooks fire *inside* the mailbox mutex critical
    /// section, so their channels are necessarily synchronous. Two of the
    /// parties that send those tokens — the Agent Loop's safe-boundary
    /// drain and the detached background runner's terminal enqueue — park
    /// on those channels from Tokio tasks, which blocks their worker
    /// threads for the duration of each handshake. If the controller also
    /// waited synchronously from a Tokio task, it would occupy a third
    /// worker while waiting for a task that still needs one, making
    /// progress a function of how much CPU the runtime happens to get. Each
    /// controller receive therefore runs on the blocking pool via
    /// [`recv_probe_token`], and every step waits exactly rather than for a
    /// bounded time.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[allow(clippy::too_many_lines)]
    async fn terminal_inbound_after_snapshot_can_never_join_the_first_batch() {
        use crate::runtime::inbound::MailboxProbe;
        use std::sync::mpsc::sync_channel;
        // One snapshot token per non-empty drain (two: [human], then
        // [terminal]) and one release token per parked drain; the final
        // empty drain returns before the probe hooks fire. One
        // computed/resume token pair per enqueue (human + terminal).
        let (snapshot_tx, snapshot_rx) = sync_channel(2);
        let (release_tx, release_rx) = sync_channel(2);
        let (computed_tx, computed_rx) = sync_channel(1);
        let (resume_tx, resume_rx) = sync_channel(1);
        let mailbox = crate::runtime::inbound::ConversationInboundMailbox::with_probe(
            ConversationId::new("conv-1"),
            MailboxProbe {
                drain_snapshot: Some(snapshot_tx),
                drain_release: Some(release_rx),
                enqueue_computed: Some(computed_tx),
                enqueue_resume: Some(resume_rx),
            },
        );
        // The human message is enqueued through the probe: sequence 1 is
        // computed and published only after the test releases the enqueue.
        let enqueueing = mailbox.clone();
        let human_task = tokio::task::spawn_blocking(move || {
            enqueueing
                .enqueue(inbound_message("msg-human", "hello"))
                .expect("enqueue human")
        });
        let computed_rx = recv_probe_token(computed_rx, "human enqueue sequence computed").await;
        resume_tx.send(()).expect("release the human enqueue");
        human_task.await.expect("human enqueue task");

        let call = ToolCall {
            id: ToolCallId::new("call-bg"),
            tool_id: ToolId::new("tool-bg"),
            name: "bg".to_owned(),
            arguments: serde_json::json!({"__rustx_execution": "background"}),
        };
        let adapter = Arc::new(ScriptedAdapter::new(vec![
            tool_call_script(&call),
            vec![
                ModelEvent::Started,
                ModelEvent::TextDelta {
                    block_index: ContentBlockIndex::new(0),
                    text: "turn two".to_owned(),
                },
                ModelEvent::Completed {
                    finish_reason: ModelFinishReason::Stop,
                    usage: None,
                },
            ],
            vec![
                ModelEvent::Started,
                ModelEvent::TextDelta {
                    block_index: ContentBlockIndex::new(0),
                    text: "turn three".to_owned(),
                },
                ModelEvent::Completed {
                    finish_reason: ModelFinishReason::Stop,
                    usage: None,
                },
            ],
        ]));
        let (tool, mut started, release) = ParkingBackgroundTool::new();
        let mut tools = ToolRegistry::new();
        tools
            .register(tool.definition.clone(), Arc::new(tool))
            .expect("register bg tool");
        let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
        let tool_runtime = tool_runtime_with_mailbox(Some(mailbox.clone()));
        let (pause, mut pause_reached, pause_release) = ContinuationBoundaryPause::install();
        let controller = tokio::spawn(async move {
            // 1. Drain #1 committed its finite snapshot [human] and is
            //    parked inside its critical section, still holding the
            //    mailbox mutex. The first batch is fixed forever.
            let snapshot_rx = recv_probe_token(snapshot_rx, "first drain snapshot committed").await;
            // 2. The detached background runner is provably started; settle
            //    it. Its terminal enqueue can only block on the mailbox
            //    mutex that drain #1 still owns.
            started
                .wait_for(|started| *started)
                .await
                .expect("bg runner started");
            release.notify_one();
            // 3. Release drain #1: the [human] batch is appended, turn 1
            //    completes, and the loop reaches the continuation boundary.
            release_tx.send(()).expect("release the first drain");
            // 4. The Agent Loop is parked after turn 1 and before turn 2:
            //    no further mailbox drain can occur until it is released.
            pause_reached
                .wait_for(|reached| *reached)
                .await
                .expect("continuation boundary reached");
            // 5. The terminal enqueue owns the mailbox mutex: its sequence
            //    is computed and the item is not yet published. This is
            //    provably after drain #1 released the mutex (step 3) and
            //    before any later drain (the loop is parked, step 4).
            let _computed_rx =
                recv_probe_token(computed_rx, "terminal enqueue owns the mailbox mutex").await;
            // 6. Publish the terminal under that mutex, pre-buffer the
            //    release token for drain #2, and release the Agent Loop:
            //    the next safe-boundary drain must now observe [terminal].
            resume_tx.send(()).expect("publish the terminal inbound");
            release_tx.send(()).expect("release the second drain");
            pause_release
                .send(())
                .expect("release the continuation boundary");
            // 7. Drain #2 committed its finite snapshot [terminal]; release
            //    the second continuation boundary (parked after turn 2) so
            //    the terminal-observing turn can run.
            let _snapshot_rx =
                recv_probe_token(snapshot_rx, "second drain snapshot committed").await;
            pause_release
                .send(())
                .expect("release the second continuation boundary");
        });
        let (_dir, _coordinator, lease) = capability_lease(tools, &tool_runtime).await;
        let execution = AgentExecution::new(
            request(&adapter),
            lease,
            &cancellation,
            runtime(&adapter),
            &tool_runtime,
        )
        .expect("conversation identity matches the tool runtime");
        execution
            .continuation_pause
            .lock()
            .expect("continuation pause lock")
            .replace(pause);
        // Outer liveness guards only: every step above is an exact
        // handshake, so these bounds never participate in the proof — they
        // exist so a regression fails loudly instead of hanging.
        let _result = tokio::time::timeout(LIVENESS_GUARD, execution.run())
            .await
            .expect("the attempt terminates");
        tokio::time::timeout(LIVENESS_GUARD, controller)
            .await
            .expect("the controller terminates")
            .expect("controller task");

        let requests = adapter.requests.lock().expect("requests lock").clone();
        assert_eq!(
            requests.len(),
            3,
            "the constructed ordering is exactly: human turn, terminal turn, stop turn"
        );
        let second_request = &requests[1];
        assert!(
            second_request.messages.iter().any(|message| {
                matches!(message, MessageBlock::User(user) if user.id == MessageId::new("msg-human"))
            }),
            "the human message joins the second request"
        );
        assert!(
            !second_request.messages.iter().any(|message| {
                matches!(message, MessageBlock::User(user) if user.id.as_str() == "background-exec_1-terminal")
            }),
            "the terminal can never appear in the first drained batch"
        );
        assert!(
            requests[2]
                .messages
                .iter()
                .any(|message| matches!(message, MessageBlock::User(user) if user.id.as_str() == "background-exec_1-terminal")),
            "the terminal inbound waits for the next drained batch"
        );
        let terminal_occurrences = requests
            .iter()
            .flat_map(|request| &request.messages)
            .filter(|message| {
                matches!(message, MessageBlock::User(user) if user.id.as_str() == "background-exec_1-terminal")
            })
            .count();
        assert_eq!(
            terminal_occurrences, 1,
            "the terminal inbound is drained and committed exactly once"
        );
        assert!(mailbox.drain().is_none(), "the mailbox is drained");
    }
}
