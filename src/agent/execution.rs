//! The deterministic agent execution loop (M3 + M4 context integration).
//!
//! The loop turns one canonical `ModelEvent` stream into an attempt
//! execution:
//!
//! ```text
//! input
//!  ↓
//! canonical history + latest context checkpoint + pending FreshInboundTurn
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
//! invocation that observes it. The conversation inbound mailbox is opt-in
//! via [`AgentExecution::with_inbound_mailbox`], which adds the mailbox
//! safe-boundary rules. Cancellation is a generic Agent Loop invariant for
//! every execution: observable cancellation is checked before every model
//! turn begins.

use futures_util::StreamExt;

use crate::context::engine::CompactionConstraints;
use crate::context::error::{ContextError, ContextErrorKind};
use crate::context::projection::ContextProjection;
use crate::context::status::AgentStatusAttachment;
use crate::context::tokens::ProviderObservedInput;
use crate::context::{ContextRuntime, compile_projection};
use crate::events::types::{AttemptFailure, AttemptOutcome, RuntimeEvent};
use crate::message::types::{AgentMessageBlock, MessageBlock, ToolMessageBlock};
use crate::model::adapter::{ModelAdapter, ModelEventStream};
use crate::model::error::{ModelError, ModelErrorKind};
use crate::model::event::ModelEvent;
use crate::model::finish::ModelFinishReason;
use crate::model::types::{ModelProtocol, ModelRequest, ModelUsage, ReasoningEffort};
use crate::runtime::continuation::ProviderContinuationState;
use crate::runtime::identity::{AgentId, AttemptId, ConversationId, MessageId};
use crate::runtime::inbound::{ConversationInboundMailbox, FreshInboundTurn, MailboxError};
use crate::runtime::types::{CancellationReason, RuntimeError};
use crate::tools::executor::ToolRegistry;
use crate::tools::types::ToolCall;

use super::assembly::ModelEventAssembler;
use super::cancellation::AgentCancellation;
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
#[derive(Debug, Clone, PartialEq)]
pub struct AgentExecutionRequest {
    /// The agent being executed.
    pub agent_id: AgentId,
    /// The conversation the attempt belongs to.
    pub conversation_id: ConversationId,
    /// The attempt identity reported by attempt-level events.
    pub attempt_id: AttemptId,
    /// The conversation/input state the attempt starts from. The loop owns
    /// the committed history and appends agent and tool messages to it.
    pub initial_messages: Vec<MessageBlock>,
    /// The explicit fresh inbound trigger of the attempt's first turn, when
    /// the attempt starts with inbound material the model has not yet
    /// observed. Fresh inbound identity is explicit execution state, never
    /// inferred from message role or history shape; omitting it is only
    /// meaningful when no inbound material exists (a pure continuation
    /// attempt) and is never an Agent Status disable switch.
    pub initial_fresh_inbound: Option<FreshInboundTurn>,
    /// The per-execution/conversation IANA timezone metadata used by the
    /// temporal Agent Status section, when known. The process/system local
    /// timezone is never consulted.
    pub timezone: Option<Tz>,
    /// Provider model identifier for every model request of the attempt.
    pub model: String,
    /// Canonical protocol every model request must use.
    pub protocol: ModelProtocol,
    /// Reasoning effort for every model request.
    pub reasoning: ReasoningEffort,
    /// The runtime-resolved effective maximum output tokens.
    pub max_output_tokens: u32,
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
/// `messages` is canonical history only: the initial canonical messages
/// plus every committed agent message, tool message, and drained inbound
/// user message of the attempt. No compaction summary and no projection-only
/// agent slice ever appears here.
#[derive(Debug, Clone, PartialEq)]
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
    /// The committed conversation state: the initial messages plus every
    /// committed agent message, tool message, and drained inbound user
    /// message of the attempt. This is canonical history, never a
    /// projection.
    pub messages: Vec<MessageBlock>,
}

/// One agent attempt execution.
///
/// The loop borrows the model adapter, the immutable tool registry, the
/// attempt cancellation signal, and owns the mandatory M4 context runtime,
/// the execution state machine, the committed history, the retained
/// continuation state, the pending fresh inbound trigger, the conversation
/// inbound mailbox handle (when attached), and the runtime event trace.
pub struct AgentExecution<'a> {
    request: AgentExecutionRequest,
    adapter: &'a dyn ModelAdapter,
    tools: &'a ToolRegistry,
    cancellation: &'a AgentCancellation,
    state: ExecutionStateMachine,
    history: Vec<MessageBlock>,
    events: Vec<RuntimeEvent>,
    pending_continuation: Option<ProviderContinuationState>,
    /// The committed agent message that established the pending
    /// continuation, when one is pending.
    continuation_owner: Option<MessageId>,
    /// The pending fresh inbound turn: `Some` until a successful model
    /// invocation has observed it. One pending fresh inbound turn produces
    /// at most one Agent Status snapshot per request preparation.
    pending_fresh_inbound: Option<FreshInboundTurn>,
    context_runtime: ContextRuntime<'a>,
    observed: Option<ProviderObservedInput>,
    last_request_fingerprint: Option<u64>,
    /// The conversation inbound mailbox, when attached. The mailbox stays
    /// external to the execution: the execution object is a consumer that
    /// performs one finite drain per safe boundary, never the authoritative
    /// conversation queue store.
    mailbox: Option<ConversationInboundMailbox>,
    /// Test-only control point parked at the turn-continuation boundary:
    /// after a completed turn (and all its mailbox drain/append work)
    /// returned "continue", before the generic cancellation check of the
    /// next model turn; never present outside `#[cfg(test)]`.
    #[cfg(test)]
    continuation_pause: Option<test_sync::ContinuationBoundaryPause>,
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

/// The terminal outcome of the whole attempt.
enum Terminal {
    Completed { finish_reason: ModelFinishReason },
    Cancelled { reason: CancellationReason },
    Failed { failure: AttemptFailure },
}

impl<'a> AgentExecution<'a> {
    /// Creates an attempt execution over the given adapter, tool registry,
    /// cancellation signal, and the mandatory M4 context runtime.
    ///
    /// The context runtime is required: there is exactly one normal
    /// execution model — canonical history is always projected through the
    /// context engine, and Agent Status is composed whenever a pending fresh
    /// inbound turn exists. There is no no-context mode and no Agent Status
    /// disable flag.
    #[must_use]
    pub fn new(
        request: AgentExecutionRequest,
        adapter: &'a dyn ModelAdapter,
        tools: &'a ToolRegistry,
        cancellation: &'a AgentCancellation,
        context_runtime: ContextRuntime<'a>,
    ) -> Self {
        Self {
            history: request.initial_messages.clone(),
            request,
            adapter,
            tools,
            cancellation,
            state: ExecutionStateMachine::new(),
            events: Vec::new(),
            pending_continuation: None,
            continuation_owner: None,
            pending_fresh_inbound: None,
            context_runtime,
            observed: None,
            last_request_fingerprint: None,
            mailbox: None,
            #[cfg(test)]
            continuation_pause: None,
            turn: 0,
            terminal_emitted: false,
        }
    }

    /// Attaches the conversation inbound mailbox of the attempt's
    /// conversation.
    ///
    /// The mailbox is conversation-owned coordination that stays external to
    /// the execution: at every safe turn boundary the loop performs exactly
    /// one finite atomic drain and appends every drained inbound message as
    /// its own canonical `UserMessageBlock` before the next model request.
    /// The drained batch becomes one new `FreshInboundTurn`, so the next
    /// model request carries exactly one Agent Status snapshot targeting the
    /// final drained message. The mailbox must belong to the same
    /// conversation as the request; a mismatched conversation is rejected
    /// explicitly.
    ///
    /// # Errors
    ///
    /// Returns [`MailboxError::ConversationMismatch`] when the mailbox
    /// belongs to a different conversation than the request.
    pub fn with_inbound_mailbox(
        mut self,
        mailbox: ConversationInboundMailbox,
    ) -> Result<Self, MailboxError> {
        if mailbox.conversation_id() != &self.request.conversation_id {
            return Err(MailboxError::ConversationMismatch {
                expected: self.request.conversation_id.clone(),
                actual: mailbox.conversation_id().clone(),
            });
        }
        self.mailbox = Some(mailbox);
        Ok(self)
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
            // the first successful model invocation observes it.
            self.pending_fresh_inbound = self.request.initial_fresh_inbound.clone();
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
                if terminal.is_none() {
                    if let Some(pause) = &self.continuation_pause {
                        pause.park_at_continuation_boundary();
                    }
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
            messages: self.history,
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
        let agent_message_id =
            MessageId::new(format!("{}-agent-{}", self.request.attempt_id, self.turn));
        self.emit(RuntimeEvent::TurnStarted);

        let request = match self.prepare_model_request().await {
            Ok(request) => request,
            Err(terminal) => return Some(terminal),
        };
        self.emit(RuntimeEvent::ModelRequestStarted {
            model: request.model.clone(),
        });
        let mut invocation = match self.consume_invocation(request, &agent_message_id).await {
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
        agent_message_id: MessageId,
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
            self.continuation_owner = Some(agent_message_id.clone());
        } else {
            self.continuation_owner = None;
        }
        let has_tool_calls = !turn_assembly.tool_calls.is_empty();
        if let Err(error) = self.state.model_finished(has_tool_calls) {
            return Some(Terminal::Failed {
                failure: AttemptFailure::Runtime { error },
            });
        }
        self.commit_agent_message(&agent_message_id, &turn_assembly.content);
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
        if let Some(terminal) = self.execute_tools(&turn_assembly.tool_calls).await {
            return Some(terminal);
        }
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
        self.emit(RuntimeEvent::TurnCompleted);
        // Safe boundary after a structurally complete tool turn: every
        // foreground call of the turn executed in deterministic order and
        // every ToolMessage was committed before this point. One finite
        // mailbox drain may attach an inbound batch to the continuation;
        // the drain never splits the tool-result batch.
        self.safe_boundary_drain().err()
    }

    /// The mailbox-specific safe boundary: exactly one finite inbound
    /// mailbox snapshot after the current turn is structurally complete.
    ///
    /// This function is mailbox-owned semantics only, separate from the
    /// generic Agent Loop cancellation checkpoint (which lives in `run()`
    /// before every model turn). With no mailbox attached there is no
    /// snapshot work: the function returns `Ok(false)` immediately and
    /// observes no mailbox state. With a mailbox attached, cancellation
    /// wins before batch selection: when cancellation is already
    /// observable, no drain happens, all pending items stay in the
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
    /// when no mailbox is attached or the snapshot observed an empty
    /// mailbox, and the attempt terminal when cancellation was observable
    /// before the snapshot.
    fn safe_boundary_drain(&mut self) -> Result<bool, Terminal> {
        let Some(mailbox) = &self.mailbox else {
            return Ok(false);
        };
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
            message_ids.push(item.message().id.clone());
            self.history.push(MessageBlock::User(item.into_message()));
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
    /// `ModelRequest`: canonical history plus the latest checkpoint flow
    /// through the context engine into a projection, and the projection is
    /// compiled into the request messages. The pending fresh inbound turn
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
        let should_compact = match self
            .context_runtime
            .engine
            .should_compact(&projection, self.request.max_output_tokens)
        {
            Ok(value) => value,
            Err(error) => return Err(Self::context_failure_terminal(&error)),
        };
        if should_compact {
            // Successful compaction invalidates any pending continuation.
            let must_cover = self.continuation_owner.clone();
            let fresh = self.pending_fresh_inbound.clone();
            match self
                .perform_compaction(must_cover.as_ref(), fresh.as_ref(), status.as_ref(), None)
                .await
            {
                Ok(()) => {}
                Err(terminal) => return Err(terminal),
            }
            self.pending_continuation = None;
            self.continuation_owner = None;
            self.observed = None;
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
        fresh.validate_against(&self.history).map_err(|error| {
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
        };
        let status = self.context_runtime.status_composer.compose(&context)?;
        Ok(Some(AgentStatusAttachment {
            target_message_id,
            rendered: crate::context::status::render_agent_status(&status),
        }))
    }

    /// The persisted timestamp of one committed inbound message.
    fn inbound_time_of(&self, message_id: &MessageId) -> Option<DateTime<Utc>> {
        self.history.iter().find_map(|message| match message {
            MessageBlock::User(user) if &user.id == message_id => user.timestamp,
            _ => None,
        })
    }

    /// Builds the model request from the current projection of the latest
    /// checkpoint.
    fn context_model_request(
        &mut self,
        status: Option<&AgentStatusAttachment>,
    ) -> Result<ModelRequest, Terminal> {
        let projection = self.current_projection(status)?;
        self.last_request_fingerprint = Some(projection.fingerprint());
        Ok(self.model_request_from_projection(&projection))
    }

    /// The current projection of the latest checkpoint, or the terminal the
    /// attempt must settle with when the context plane failed.
    fn current_projection(
        &self,
        agent_status: Option<&AgentStatusAttachment>,
    ) -> Result<ContextProjection, Terminal> {
        let checkpoint = self
            .context_runtime
            .checkpoint_store
            .load(&self.request.conversation_id)
            .map_err(|error| Self::context_failure_terminal(&error))?;
        self.context_runtime
            .engine
            .build_projection(
                &self.history,
                checkpoint.as_ref(),
                &self.tools.definitions(),
                self.observed.as_ref(),
                agent_status,
            )
            .map_err(|error| Self::context_failure_terminal(&error))
    }

    /// The `AttemptFailed` terminal of a context-plane failure that occurred
    /// before any compaction began.
    fn context_failure_terminal(error: &ContextError) -> Terminal {
        Terminal::Failed {
            failure: AttemptFailure::Runtime {
                error: RuntimeError::ContextCompactionFailed {
                    message: error.message.clone(),
                },
            },
        }
    }

    /// Runs one compaction: plan, summarize, verify progress, commit the
    /// checkpoint, and rebuild the projection.
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
    /// against the pending summary, checked again before the checkpoint
    /// commit, and checked again before any retry by the callers: once
    /// cancellation is observable, no summary, no checkpoint, and no retry
    /// progress may begin, and the pending summary future is dropped.
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
            Ok(()) => {}
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
        self.emit(RuntimeEvent::CompactionCompleted);
        Ok(())
    }

    /// The cancellation-aware compaction pipeline: plan, summarize, verify
    /// progress, apply, fit-check, and persist.
    ///
    /// Cancellation is observed before the compaction, raced (biased)
    /// against the pending summary, and checked again before the checkpoint
    /// commit: once cancellation is observable, no summary, no checkpoint,
    /// and no retry progress may begin, and the pending summary future is
    /// dropped.
    async fn run_compaction(
        &self,
        must_cover_through: Option<&MessageId>,
        fresh_inbound: Option<&FreshInboundTurn>,
        agent_status: Option<&AgentStatusAttachment>,
    ) -> Result<(), ContextError> {
        if self.cancellation.is_cancelled() {
            return Err(ContextError::new(
                ContextErrorKind::Cancelled,
                "compaction cancelled before it began",
            ));
        }
        let tools = self.tools.definitions();
        let checkpoint = self
            .context_runtime
            .checkpoint_store
            .load(&self.request.conversation_id)?;
        let projection = self.context_runtime.engine.build_projection(
            &self.history,
            checkpoint.as_ref(),
            &tools,
            self.observed.as_ref(),
            agent_status,
        )?;
        let plan = self.context_runtime.engine.plan_compaction(
            &self.history,
            checkpoint.as_ref(),
            &projection,
            &tools,
            self.request.max_output_tokens,
            &CompactionConstraints {
                must_cover_through,
                fresh_inbound,
            },
        )?;
        let summary_request = self.context_runtime.engine.summary_request(
            &self.history,
            checkpoint.as_ref(),
            &plan,
        )?;
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
        // Cancellation after the summary returned but before the checkpoint
        // commit: no checkpoint is saved, no completion is emitted.
        if self.cancellation.is_cancelled() {
            return Err(ContextError::new(
                ContextErrorKind::Cancelled,
                "compaction cancelled before the checkpoint commit",
            ));
        }
        let (checkpoint, projection) = self.context_runtime.engine.apply_compaction(
            &self.request.conversation_id,
            &self.history,
            checkpoint.as_ref(),
            &plan,
            &summary_text,
            &tools,
        )?;
        // The rebuilt projection must fit under the soft input limit; if
        // pinned context and the actual summary cannot fit, fail explicitly.
        match self
            .context_runtime
            .engine
            .fits_under_soft_limit(&projection, self.request.max_output_tokens)
        {
            Ok(true) => {}
            Ok(false) => {
                return Err(ContextError::new(
                    ContextErrorKind::CannotFit,
                    "compacted projection still exceeds the soft input limit",
                ));
            }
            Err(error) => return Err(error),
        }
        // The checkpoint commit point: save before emitting
        // CompactionCompleted, so the event means the new checkpoint is
        // committed to the M4 checkpoint store.
        self.context_runtime.checkpoint_store.save(&checkpoint)?;
        Ok(())
    }

    /// Emits `CompactionFailed` with the diagnostic and returns the
    /// compaction terminal for this caller.
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
            None => Self::context_failure_terminal(error),
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
        let mut stream = self
            .adapter
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
        // A successful compaction establishes a new context boundary: the
        // old opaque provider continuation must never pair with the new
        // projection, and it is never inspected or transformed.
        self.pending_continuation = None;
        self.continuation_owner = None;
        self.observed = None;
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
            model: request.model.clone(),
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
        agent_message_id: &MessageId,
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
                        self.emit_model_event(&event, agent_message_id);
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

    /// Executes the turn's tool calls in deterministic block order.
    ///
    /// Returns a terminal outcome when the attempt must settle: an unknown
    /// tool (no result exists, so the attempt fails explicitly) or a
    /// cancellation observed while waiting for a tool. A failing tool still
    /// produces a normalized [`ToolExecutionResult`] with a failure status
    /// and is passed back to the model like any other result.
    ///
    /// Tool execution races against attempt cancellation: once cancellation
    /// is observable, the loop stops awaiting the tool, drops the pending
    /// tool future, records no completion and no tool message, executes no
    /// later call, and settles cancelled. Dropping the future does not
    /// guarantee that external work is physically killed; executor-specific
    /// cancellation is a later milestone.
    ///
    /// [`ToolExecutionResult`]: crate::tools::types::ToolExecutionResult
    async fn execute_tools(&mut self, calls: &[ToolCall]) -> Option<Terminal> {
        for call in calls {
            if self.cancellation.is_cancelled() {
                return Some(Terminal::Cancelled {
                    reason: self.cancellation.reason(),
                });
            }
            // An unresolved tool has no executable and no result: the
            // attempt fails with the typed UnknownTool runtime error. No
            // tool-execution event is emitted because no tool executed.
            let Some(tool) = self.tools.resolve(call) else {
                return Some(Terminal::Failed {
                    failure: AttemptFailure::Runtime {
                        error: RuntimeError::UnknownTool {
                            name: call.name.clone(),
                        },
                    },
                });
            };
            let tool_id = tool.definition().id.clone();
            self.emit(RuntimeEvent::ToolExecutionStarted {
                tool_call_id: call.id.clone(),
                tool_id: tool_id.clone(),
            });
            // The race is biased: when both the cancellation signal and the
            // tool future are ready, cancellation wins deterministically, so
            // cancellation always prevents new execution progress once it is
            // observable.
            let result = tokio::select! {
                biased;
                () = self.cancellation.cancelled() => {
                    return Some(Terminal::Cancelled {
                        reason: self.cancellation.reason(),
                    });
                }
                result = tool.execute(call) => result,
            };
            self.emit(RuntimeEvent::ToolExecutionCompleted {
                tool_call_id: call.id.clone(),
                tool_id: tool_id.clone(),
                result: result.clone(),
            });
            self.history.push(MessageBlock::Tool(ToolMessageBlock {
                id: MessageId::new(format!(
                    "{}-tool-{}-{}",
                    self.request.attempt_id, self.turn, call.id
                )),
                tool_call_id: call.id.clone(),
                tool_id: tool_id.clone(),
                result,
            }));
        }
        None
    }

    /// Builds the canonical request from a compiled context projection.
    ///
    /// Projection-only agent slices are materialized transiently under their
    /// original source `MessageId` as a model-context view; they are never
    /// authoritative ledger content. The ephemeral Agent Status attachment
    /// of the projection travels alongside the compiled messages; it is
    /// never encoded as a fake canonical message.
    fn model_request_from_projection(&self, projection: &ContextProjection) -> ModelRequest {
        let compiled = compile_projection(projection);
        ModelRequest {
            model: self.request.model.clone(),
            protocol: self.request.protocol,
            messages: compiled.messages,
            tools: self.tools.definitions(),
            agent_status: compiled.agent_status,
            reasoning: self.request.reasoning,
            max_output_tokens: self.request.max_output_tokens,
            continuation: self.pending_continuation.clone(),
        }
    }

    /// Commits the assembled agent message into the conversation state.
    fn commit_agent_message(
        &mut self,
        message_id: &MessageId,
        content: &[crate::message::types::AgentContentBlock],
    ) {
        self.history.push(MessageBlock::Agent(AgentMessageBlock {
            id: message_id.clone(),
            content: content.to_vec(),
        }));
    }

    /// Emits the runtime events for one non-terminal model event.
    fn emit_model_event(&mut self, event: &ModelEvent, message_id: &MessageId) {
        match event {
            ModelEvent::Started => {
                self.emit(RuntimeEvent::AgentMessageStarted {
                    message_id: message_id.clone(),
                });
            }
            ModelEvent::TextDelta { block_index, text } => {
                self.emit(RuntimeEvent::AgentTextDelta {
                    message_id: message_id.clone(),
                    block_index: *block_index,
                    delta: text.clone(),
                });
            }
            ModelEvent::ReasoningDelta { block_index, text } => {
                self.emit(RuntimeEvent::AgentReasoningDelta {
                    message_id: message_id.clone(),
                    block_index: *block_index,
                    delta: text.clone(),
                });
            }
            ModelEvent::RefusalDelta { block_index, text } => {
                self.emit(RuntimeEvent::AgentRefusalDelta {
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
        self.events.push(event);
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
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    use futures_util::future::BoxFuture;

    use crate::message::types::{
        ContentBlockIndex, MessageBlock, UserContentBlock, UserMessageBlock, UserSource,
    };
    use crate::model::adapter::{ModelAdapter, ModelEventStream};
    use crate::model::event::ModelEvent;
    use crate::model::finish::ModelFinishReason;
    use crate::model::types::{ModelProtocol, ModelRequest, ReasoningEffort};
    use crate::model::{ModelCancellation, chat_protocol};
    use crate::runtime::identity::{
        AgentId, AttemptId, ConversationId, MessageId, ToolCallId, ToolId,
    };
    use crate::runtime::inbound::ConversationInboundMailbox;
    use crate::runtime::types::CancellationReason;
    use crate::tools::executor::{Tool, ToolRegistry};
    use crate::tools::types::{
        ToolCall, ToolCallStart, ToolDefinition, ToolExecutionMode, ToolExecutionResult,
        ToolExecutionStatus, ToolOrigin, ToolReplayPolicy,
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
            _cancellation: ModelCancellation,
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

    /// An instant fake tool returning one fixed successful result.
    struct InstantTool {
        definition: ToolDefinition,
    }

    impl InstantTool {
        fn new(id: &str, name: &str) -> Self {
            Self {
                definition: ToolDefinition {
                    id: ToolId::new(id),
                    name: name.to_owned(),
                    description: String::new(),
                    input_schema: serde_json::json!({"type": "object"}),
                    execution_mode: ToolExecutionMode::Sequential,
                    replay_policy: ToolReplayPolicy::Never,
                    origin: ToolOrigin::Builtin,
                },
            }
        }
    }

    impl Tool for InstantTool {
        fn definition(&self) -> &ToolDefinition {
            &self.definition
        }

        fn execute<'a>(&'a self, _call: &'a ToolCall) -> BoxFuture<'a, ToolExecutionResult> {
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

    fn request() -> AgentExecutionRequest {
        AgentExecutionRequest {
            agent_id: AgentId::new("agent-a"),
            conversation_id: ConversationId::new("conv-1"),
            attempt_id: AttemptId::new("attempt-1"),
            initial_messages: Vec::new(),
            initial_fresh_inbound: None,
            timezone: None,
            model: "scripted-model".to_owned(),
            protocol: chat_protocol(),
            reasoning: ReasoningEffort::Medium,
            max_output_tokens: 512,
        }
    }

    /// A deterministic context runtime with a window far larger than any
    /// scripted request, so no compaction ever triggers in these tests.
    fn runtime() -> ContextRuntime<'static> {
        use crate::context::checkpoint::InMemoryCheckpointStore;
        use crate::context::summarizer::{ContextSummarizer, SummaryRequest};
        use crate::context::{
            ContextConfig, ContextEngine, ContextError, DefaultTokenEstimator, TokenEstimator,
        };
        use crate::model::ModelCancellation;
        struct NeverSummarizes;
        impl ContextSummarizer for NeverSummarizes {
            fn summarize(
                &self,
                _request: SummaryRequest,
                _cancellation: ModelCancellation,
            ) -> futures_util::future::BoxFuture<'_, Result<String, ContextError>> {
                unreachable!("no compaction is possible under a huge window")
            }
        }
        let estimator: Arc<dyn TokenEstimator> = Arc::new(DefaultTokenEstimator);
        let engine = ContextEngine::new(
            ContextConfig {
                context_window_tokens: 10_000_000,
                reserve_tokens: 0,
                keep_recent_tokens: 0,
            },
            estimator,
        )
        .expect("valid context configuration");
        ContextRuntime::new(
            engine,
            Arc::new(NeverSummarizes),
            Arc::new(InMemoryCheckpointStore::new()),
        )
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
                model: "scripted-model".to_owned(),
            },
            RuntimeEvent::AgentMessageStarted {
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
        let adapter = ScriptedAdapter::new(vec![tool_call_script(&call)]);
        let mut tools = ToolRegistry::new();
        tools.insert(InstantTool::new("tool-alpha", "alpha"));
        let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
        let (pause, reached_rx, release_tx) = ContinuationBoundaryPause::install();
        let controller = boundary_controller(reached_rx, release_tx, cancellation.clone());

        let mut execution =
            AgentExecution::new(request(), &adapter, &tools, &cancellation, runtime());
        execution.continuation_pause = Some(pause);
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
        let adapter = ScriptedAdapter::new(vec![tool_call_script(&call)]);
        let mut tools = ToolRegistry::new();
        tools.insert(InstantTool::new("tool-alpha", "alpha"));
        let mailbox = ConversationInboundMailbox::new(ConversationId::new("conv-1"));
        mailbox
            .enqueue(inbound_message("msg-a", "A"))
            .expect("enqueue A before the attempt");
        let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
        let (pause, reached_rx, release_tx) = ContinuationBoundaryPause::install();
        let controller = boundary_controller(reached_rx, release_tx, cancellation.clone());

        let mut execution =
            AgentExecution::new(request(), &adapter, &tools, &cancellation, runtime());
        execution.continuation_pause = Some(pause);
        let result = execution
            .with_inbound_mailbox(mailbox.clone())
            .expect("mailbox belongs to the request conversation")
            .run()
            .await;
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
            .messages
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
}
