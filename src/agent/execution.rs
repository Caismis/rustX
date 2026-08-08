//! The deterministic agent execution loop (M3 + M4 context integration).
//!
//! The loop turns one canonical `ModelEvent` stream into an attempt
//! execution:
//!
//! ```text
//! input
//!  ↓
//! canonical history + latest context checkpoint
//!  ↓
//! ContextEngine → ContextProjection (M4, opt-in)
//!  ↓
//! model request (projection + tools + continuation state)
//!  ↓
//! canonical model events
//!  ↓
//! message assembly + RuntimeEvent emission
//!  ↓
//! tool calls (if requested): resolve, execute, record
//!  ↓
//! continuation (or proactive compaction / compact-and-retry on overflow)
//!  ↓
//! exactly one terminal RuntimeEvent
//! ```
//!
//! Ownership: the loop owns execution semantics, message assembly, tool
//! execution, continuation state, cancellation observation, context
//! projection integration, and the runtime event trace. The adapter owns
//! provider protocol translation only. No provider protocol concept appears
//! in this module.
//!
//! The M4 context path is opt-in via [`AgentExecution::with_context_runtime`];
//! `AgentExecution::new` remains the explicit no-context/unbounded
//! compatibility path.

use futures_util::StreamExt;

use crate::context::error::{ContextError, ContextErrorKind};
use crate::context::projection::ContextProjection;
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
use crate::runtime::types::{CancellationReason, RuntimeError};
use crate::tools::executor::ToolRegistry;
use crate::tools::types::ToolCall;

use super::assembly::ModelEventAssembler;
use super::cancellation::AgentCancellation;
use super::state::{ExecutionState, ExecutionStateMachine};

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
/// `messages` is canonical history only: initial messages plus committed
/// agent and tool messages. No compaction summary and no projection-only
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
    /// committed agent and tool message of the attempt. This is canonical
    /// history, never a projection.
    pub messages: Vec<MessageBlock>,
}

/// One agent attempt execution.
///
/// The loop borrows the model adapter, the immutable tool registry, and the
/// attempt cancellation signal, and owns the execution state machine, the
/// committed history, the retained continuation state, the M4 context
/// runtime (when enabled), and the runtime event trace.
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
    context_runtime: Option<ContextRuntime<'a>>,
    observed: Option<ProviderObservedInput>,
    last_request_fingerprint: Option<u64>,
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
    /// and cancellation signal.
    ///
    /// This is the explicit no-context/unbounded compatibility path: without
    /// [`AgentExecution::with_context_runtime`], every model request carries
    /// the full committed history exactly as M3 defined it.
    #[must_use]
    pub fn new(
        request: AgentExecutionRequest,
        adapter: &'a dyn ModelAdapter,
        tools: &'a ToolRegistry,
        cancellation: &'a AgentCancellation,
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
            context_runtime: None,
            observed: None,
            last_request_fingerprint: None,
            turn: 0,
            terminal_emitted: false,
        }
    }

    /// Enables the M4 context path: the loop projects canonical history
    /// through the given context runtime before every model request, applies
    /// automatic proactive compaction, and recovers from
    /// `ContextWindowExceeded` through exactly one bounded compact-and-retry.
    ///
    /// The bundle's engine, summarizer, and checkpoint store are shared, so
    /// one checkpoint store can be reused across attempts of one
    /// conversation.
    #[must_use]
    pub fn with_context_runtime(mut self, runtime: ContextRuntime<'a>) -> Self {
        self.context_runtime = Some(runtime);
        self
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
        let terminal = match self.state.start() {
            Err(error) => Terminal::Failed {
                failure: AttemptFailure::Runtime { error },
            },
            Ok(()) => {
                if self.cancellation.is_cancelled() {
                    Terminal::Cancelled {
                        reason: self.cancellation.reason(),
                    }
                } else {
                    let mut terminal = None;
                    while terminal.is_none() {
                        terminal = self.run_turn().await;
                    }
                    terminal.expect("the attempt must settle")
                }
            }
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
            && self.context_runtime.is_some()
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
            return Some(Terminal::Completed { finish_reason });
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
        None
    }

    /// Builds the canonical request of the next model invocation.
    ///
    /// With the M4 context runtime enabled, this is the integration point
    /// immediately before every agent `ModelRequest`: canonical history plus
    /// the latest checkpoint flow through the context engine into a
    /// projection, and the projection is compiled into the request messages.
    /// Proactive automatic compaction runs when the projected input reaches
    /// the soft input limit.
    async fn prepare_model_request(&mut self) -> Result<ModelRequest, Terminal> {
        if self.context_runtime.is_none() {
            return Ok(self.model_request());
        }
        if self.cancellation.is_cancelled() {
            return Err(Terminal::Cancelled {
                reason: self.cancellation.reason(),
            });
        }
        let projection = match self.current_projection() {
            Ok(projection) => projection,
            Err(terminal) => return Err(terminal),
        };
        let should_compact = match self
            .context_runtime
            .as_ref()
            .expect("context runtime enabled")
            .engine
            .should_compact(&projection, self.request.max_output_tokens)
        {
            Ok(value) => value,
            Err(error) => return Err(Self::context_failure_terminal(&error)),
        };
        if should_compact {
            // Successful compaction invalidates any pending continuation.
            let must_cover = self.continuation_owner.clone();
            match self.perform_compaction(must_cover.as_ref(), None).await {
                Ok(()) => {}
                Err(terminal) => return Err(terminal),
            }
            self.pending_continuation = None;
            self.continuation_owner = None;
            self.observed = None;
        }
        self.context_model_request()
    }

    /// Builds the model request from the current projection of the latest
    /// checkpoint.
    fn context_model_request(&mut self) -> Result<ModelRequest, Terminal> {
        let projection = self.current_projection()?;
        self.last_request_fingerprint = Some(projection.fingerprint());
        Ok(self.model_request_from_projection(&projection))
    }

    /// The current projection of the latest checkpoint, or the terminal the
    /// attempt must settle with when the context plane failed.
    fn current_projection(&self) -> Result<ContextProjection, Terminal> {
        let runtime = self
            .context_runtime
            .as_ref()
            .expect("context runtime enabled");
        let checkpoint = runtime
            .checkpoint_store
            .load(&self.request.conversation_id)
            .map_err(|error| Self::context_failure_terminal(&error))?;
        runtime
            .engine
            .build_projection(
                &self.history,
                checkpoint.as_ref(),
                &self.tools.definitions(),
                self.observed.as_ref(),
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
    /// Cancellation is observed before the compaction, raced (biased)
    /// against the pending summary, checked again before the checkpoint
    /// commit, and checked again before any retry by the callers: once
    /// cancellation is observable, no summary, no checkpoint, and no retry
    /// progress may begin, and the pending summary future is dropped.
    async fn perform_compaction(
        &mut self,
        must_cover_through: Option<&MessageId>,
        overflow: Option<&ModelError>,
    ) -> Result<(), Terminal> {
        if self.cancellation.is_cancelled() {
            return Err(Terminal::Cancelled {
                reason: self.cancellation.reason(),
            });
        }
        self.emit(RuntimeEvent::CompactionStarted);
        let runtime = self
            .context_runtime
            .as_ref()
            .expect("context runtime enabled");
        let tools = self.tools.definitions();
        let checkpoint = match runtime.checkpoint_store.load(&self.request.conversation_id) {
            Ok(checkpoint) => checkpoint,
            Err(error) => return Err(self.compaction_failure(&error, overflow)),
        };
        let projection = match runtime.engine.build_projection(
            &self.history,
            checkpoint.as_ref(),
            &tools,
            self.observed.as_ref(),
        ) {
            Ok(projection) => projection,
            Err(error) => return Err(self.compaction_failure(&error, overflow)),
        };
        let plan = match runtime.engine.plan_compaction(
            &self.history,
            checkpoint.as_ref(),
            &projection,
            &tools,
            self.request.max_output_tokens,
            must_cover_through,
        ) {
            Ok(plan) => plan,
            Err(error) => return Err(self.compaction_failure(&error, overflow)),
        };
        let summary_request = runtime.engine.summary_request(checkpoint.as_ref(), &plan);
        let summary = tokio::select! {
            biased;
            () = self.cancellation.cancelled() => {
                return Err(Terminal::Cancelled {
                    reason: self.cancellation.reason(),
                });
            }
            result = runtime
                .summarizer
                .summarize(summary_request, self.cancellation.model_cancellation()) => result,
        };
        let summary_text = match summary {
            Ok(text) => text,
            Err(error) => return Err(self.compaction_failure(&error, overflow)),
        };
        // Cancellation after the summary returned but before the checkpoint
        // commit: no checkpoint is saved, no completion is emitted.
        if self.cancellation.is_cancelled() {
            return Err(Terminal::Cancelled {
                reason: self.cancellation.reason(),
            });
        }
        let (checkpoint, projection) = match runtime.engine.apply_compaction(
            &self.request.conversation_id,
            &self.history,
            checkpoint.as_ref(),
            &plan,
            &summary_text,
            &tools,
        ) {
            Ok(applied) => applied,
            Err(error) => return Err(self.compaction_failure(&error, overflow)),
        };
        // The rebuilt projection must fit under the soft input limit; if
        // pinned context and the actual summary cannot fit, fail explicitly.
        match runtime
            .engine
            .fits_under_soft_limit(&projection, self.request.max_output_tokens)
        {
            Ok(true) => {}
            Ok(false) => {
                let error = ContextError::new(
                    ContextErrorKind::CannotFit,
                    "compacted projection still exceeds the soft input limit",
                );
                return Err(self.compaction_failure(&error, overflow));
            }
            Err(error) => return Err(self.compaction_failure(&error, overflow)),
        }
        // The checkpoint commit point: save before emitting
        // CompactionCompleted, so the event means the new checkpoint is
        // committed to the M4 checkpoint store.
        if let Err(error) = runtime.checkpoint_store.save(&checkpoint) {
            return Err(self.compaction_failure(&error, overflow));
        }
        self.emit(RuntimeEvent::CompactionCompleted);
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
        let must_cover = self.continuation_owner.clone();
        match self
            .perform_compaction(must_cover.as_ref(), Some(overflow_error))
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
        let request = match self.context_model_request() {
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

    /// Builds the canonical request for the next model invocation on the
    /// no-context path: the full committed history, the immutable tool
    /// definitions, and the retained continuation state.
    fn model_request(&self) -> ModelRequest {
        ModelRequest {
            model: self.request.model.clone(),
            protocol: self.request.protocol,
            messages: self.history.clone(),
            tools: self.tools.definitions(),
            reasoning: self.request.reasoning,
            max_output_tokens: self.request.max_output_tokens,
            continuation: self.pending_continuation.clone(),
        }
    }

    /// Builds the canonical request from a compiled context projection.
    ///
    /// Projection-only agent slices are materialized transiently under their
    /// original source `MessageId` as a model-context view; they are never
    /// authoritative ledger content.
    fn model_request_from_projection(&self, projection: &ContextProjection) -> ModelRequest {
        ModelRequest {
            model: self.request.model.clone(),
            protocol: self.request.protocol,
            messages: compile_projection(projection),
            tools: self.tools.definitions(),
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
