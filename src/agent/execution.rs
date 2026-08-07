//! The deterministic agent execution loop (M3).
//!
//! The loop turns one canonical `ModelEvent` stream into an attempt
//! execution:
//!
//! ```text
//! input
//!  ↓
//! model request (history + tools + continuation state)
//!  ↓
//! canonical model events
//!  ↓
//! message assembly + RuntimeEvent emission
//!  ↓
//! tool calls (if requested): resolve, execute, record
//!  ↓
//! continuation with the full committed history
//!  ↓
//! exactly one terminal RuntimeEvent
//! ```
//!
//! Ownership: the loop owns execution semantics, message assembly, tool
//! execution, continuation state, cancellation observation, and the runtime
//! event trace. The adapter owns provider protocol translation only. No
//! provider protocol concept appears in this module.

use futures_util::StreamExt;

use crate::events::types::{AttemptFailure, AttemptOutcome, RuntimeEvent};
use crate::message::types::{AgentMessageBlock, MessageBlock, ToolMessageBlock};
use crate::model::adapter::{ModelAdapter, ModelEventStream};
use crate::model::error::ModelError;
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
    /// committed agent and tool message of the attempt.
    pub messages: Vec<MessageBlock>,
}

/// One agent attempt execution.
///
/// The loop borrows the model adapter, the immutable tool registry, and the
/// attempt cancellation signal, and owns the execution state machine, the
/// committed history, the retained continuation state, and the runtime
/// event trace.
pub struct AgentExecution<'a> {
    request: AgentExecutionRequest,
    adapter: &'a dyn ModelAdapter,
    tools: &'a ToolRegistry,
    cancellation: &'a AgentCancellation,
    state: ExecutionStateMachine,
    history: Vec<MessageBlock>,
    events: Vec<RuntimeEvent>,
    pending_continuation: Option<ProviderContinuationState>,
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

/// The terminal outcome of the whole attempt.
enum Terminal {
    Completed { finish_reason: ModelFinishReason },
    Cancelled { reason: CancellationReason },
    Failed { failure: AttemptFailure },
}

impl<'a> AgentExecution<'a> {
    /// Creates an attempt execution over the given adapter, tool registry,
    /// and cancellation signal.
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
            turn: 0,
            terminal_emitted: false,
        }
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

        let request = self.model_request();
        self.emit(RuntimeEvent::ModelRequestStarted {
            model: request.model.clone(),
        });
        let mut stream = self
            .adapter
            .stream(request, self.cancellation.model_cancellation());
        let mut assembler = ModelEventAssembler::new();
        let stream_terminal = match self
            .consume_model_stream(&mut assembler, &agent_message_id, &mut stream)
            .await
        {
            Ok(stream_terminal) => stream_terminal,
            Err(terminal) => return Some(terminal),
        };
        match stream_terminal {
            StreamTerminal::Failed { error } => Some(Terminal::Failed {
                failure: AttemptFailure::Model { error },
            }),
            StreamTerminal::Completed {
                finish_reason,
                usage,
            } => {
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
                self.emit(RuntimeEvent::ModelRequestCompleted {
                    finish_reason: finish_reason.clone(),
                    usage: turn_assembly.usage,
                });
                self.pending_continuation = turn_assembly.continuation;
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
        }
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

    /// Builds the canonical request for the next model invocation: the full
    /// committed history, the immutable tool definitions, and the retained
    /// continuation state.
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
