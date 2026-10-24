//! Invocation orchestration of one MCP `tools/call`.
//!
//! One admitted rustX `ToolInvocation` is driven here across its bounded
//! multi-round-trip (MRTR) lifetime: each round builds the `tools/call`
//! request, hands it to the one physical request owner
//! (`dispatch_owned_request` in `request`), and classifies the physical
//! facts into the `tools/call` vocabulary — a terminal result, an
//! `input_required` round, or a remote task handed to `task_driver`.

use std::time::Instant;

use rmcp::model::{
    CallToolRequest, CallToolRequestParams, ClientCapabilities, ClientRequest, ServerResult,
};
use rmcp::service::PeerRequestOptions;

use crate::runtime::identity::McpServerId;
use crate::tools::executor::ToolExecutionContext;
use crate::tools::types::{ToolExecutionResult, ToolExecutionStatus};

use super::outcome::{
    McpPhysicalOutcome, McpResponseOutcome, is_transport_loss, post_dispatch_cancellation_status,
};
use super::request::McpRequestOwnership;
use super::result::{failed_mcp, mcp_empty_terminal, translate_result};
use super::runtime::McpServerRuntime;
#[cfg(test)]
use super::test_sync;
use super::{bound_error, mrtr, uses_inline_lifecycle};

impl McpServerRuntime {
    /// Executes one remote `tools/call` on this transport generation.
    ///
    /// # The external-effect frontier (Issue #205)
    ///
    /// The frontier is the **exact instant
    /// [`rmcp::Peer::send_cancellable_request`] returns `Ok`**, and this is
    /// the strongest fact rustX can establish over rmcp's request path:
    ///
    /// ```text
    /// send_cancellable_request(..) -> Err
    ///     the request was never accepted by the service event loop, so it
    ///     was never serialized and never written to the transport
    ///     => before the frontier: no remote side effect was possible
    ///
    /// send_cancellable_request(..) -> Ok
    ///     the request is enqueued on the peer's outbound channel; the
    ///     service loop may already have serialized and written it
    ///     => at/after the frontier: a remote side effect may have occurred
    /// ```
    ///
    /// rmcp's `Err` is produced by exactly one condition — the outbound
    /// `mpsc` send failing because the service event loop is gone — so it
    /// proves the message was never enqueued. `Ok` proves only enqueueing:
    /// the write happens in the service loop's own spawned send task, and
    /// nothing observable to this call distinguishes "not yet written" from
    /// "written and being executed". The frontier is therefore drawn at
    /// enqueue, deliberately conservatively, rather than at any convenient
    /// polling behaviour of the response future.
    ///
    /// Past the frontier, only a **correlated remote response** — a
    /// `CallToolResult`, or a JSON-RPC error answering this request id —
    /// proves remote terminality. Everything else (transport loss, a
    /// poisoned generation, a cancellation notification rmcp acknowledged,
    /// an abandoned response channel) leaves the external outcome unknown
    /// under the Issue #202 contract.
    ///
    /// # Cancellation
    ///
    /// Cancellation intent delivered before the frontier settles the call as
    /// a proven [`ToolExecutionStatus::Cancelled`]: no request exists, so no
    /// remote effect is possible. After the frontier, the executor sends
    /// `notifications/cancelled` — the strongest cancellation the MCP
    /// protocol defines — and then **keeps the response channel** so a
    /// remote result that beat the cancellation inside rmcp still wins and
    /// is reported as proven remote terminality. Only when no correlated
    /// remote response exists is the outcome unknown; dropping the response
    /// future is never used as evidence of anything.
    pub(super) async fn call(
        &self,
        remote_name: &str,
        identity: &McpCallIdentity<'_>,
        arguments: serde_json::Value,
        context: &ToolExecutionContext<'_>,
        generation: u64,
    ) -> ToolExecutionResult {
        let mut result = Box::pin(self.drive_mrtr_invocation(
            remote_name,
            identity,
            arguments,
            context,
            generation,
        ))
        .await;
        match &mut result.status {
            ToolExecutionStatus::Failed { error }
            | ToolExecutionStatus::OutcomeUnknown { detail: error } => {
                *error = self.credentials.redact(error);
            }
            _ => {}
        }
        result
    }

    /// Drives one rustX `ToolInvocation` across its bounded MCP multi-round-trip
    /// (MRTR) lifetime (Issue #242).
    ///
    /// # The finite state machine
    ///
    /// ```text
    ///            +------------------------------------------+
    ///            |               Calling(round N)           |
    ///            +------------------------------------------+
    ///              |            |            |            |
    ///     final    |   input_   |  protocol  |  cancellation /
    ///     CallTool |   required |  failure   |  deadline
    ///     Result   |            |            |
    ///              v            v            v            v
    ///        +----------+  +----------+  +--------+  +-------------+
    ///        | Final    |  | Input    |  | Failed |  | Cancelled / |
    ///        |          |  | Required |  |        |  | TimedOut /  |
    ///        +----------+  +----------+  +--------+  | OutcomeUnk. |
    ///              |            |                    +-------------+
    ///              |            v                          |
    ///              |   one runtime-owned Questionnaire      |
    ///              |            |                           |
    ///              |            v                           |
    ///              |   Calling(round N+1)  (N+1 <= bound)   |
    ///              v                                        v
    ///        +--------------------------------------------------+
    ///        |   exactly one terminal `ToolExecutionResult`       |
    ///        +--------------------------------------------------+
    /// ```
    ///
    /// `InputRequired` is the **only** non-terminal state. It is not a
    /// `ToolResult`, not a new invocation, not a new `ToolExecutionId`, not a
    /// canonical message, and not durable state: it is transient
    /// execution-local protocol state that lives on this stack frame and is
    /// dropped the moment the invocation settles.
    ///
    /// Every round runs on the **same** admitted connection generation — the
    /// generation was resolved once, before this function — so the whole
    /// invocation keeps one server binding, one execution lease, one remote
    /// tool name, and one set of business arguments.
    async fn drive_mrtr_invocation(
        &self,
        remote_name: &str,
        identity: &McpCallIdentity<'_>,
        arguments: serde_json::Value,
        context: &ToolExecutionContext<'_>,
        generation: u64,
    ) -> ToolExecutionResult {
        let started = Instant::now();
        let serde_json::Value::Object(arguments) = arguments else {
            return failed_mcp("MCP tool arguments must be a JSON object", context, started);
        };
        let mut continuation = mrtr::McpContinuation::default();
        // The initial call is round 1, so a server may drive at most
        // `MCP_MRTR_MAX_ROUNDS - 1` continuations.
        for round in 1..=mrtr::MCP_MRTR_MAX_ROUNDS {
            let outcome = Box::pin(self.dispatch_one_mcp_round(
                remote_name,
                &arguments,
                &continuation,
                context,
                generation,
                // The invocation's own start instant, not the round's: a
                // multi-round call reports how long the whole invocation
                // took, and a one-round call is unchanged.
                started,
            ))
            .await;
            let input_required = match outcome {
                McpRoundOutcome::Terminal(result) => return result,
                McpRoundOutcome::InputRequired(input_required) => input_required,
                // The remote half of this invocation moved into a task
                // lifecycle (Issue #243). The round loop is left for good:
                // there is no `tools/call` after task creation, so the
                // original call is never replayed and no second task
                // identity can appear.
                McpRoundOutcome::Task(created) => {
                    return Box::pin(
                        self.drive_remote_task(&created, identity, context, generation, started),
                    )
                    .await;
                }
            };
            // The round bound is checked the moment an intermediate result
            // arrives, *before* any translation or interaction publication:
            // an unbounded server loop must never be able to create pending
            // human-interaction state on its way to being refused.
            if round == mrtr::MCP_MRTR_MAX_ROUNDS {
                return failed_mcp(
                    &format!(
                        "the MCP server asked for more input after {round} tools/call rounds, \
                         which is the rustX multi-round-trip bound"
                    ),
                    context,
                    started,
                );
            }
            match self
                .settle_mrtr_round(remote_name, identity, &input_required, context, started)
                .await
            {
                MrtrRoundSettlement::Continue(next) => continuation = next,
                MrtrRoundSettlement::Terminal(result) => return result,
            }
        }
        // Unreachable: the loop either returns a terminal result or refuses
        // the round above the bound. Kept total rather than panicking.
        failed_mcp(
            "the MCP multi-round-trip loop ended without a terminal result",
            context,
            started,
        )
    }

    /// Turns one intermediate `InputRequiredResult` into either the
    /// continuation of the next round or a terminal settlement.
    ///
    /// Everything human happens here, and nothing here dispatches: the caller
    /// owns the continuation dispatch frontier, so a cancellation that wins
    /// while this function runs can never be followed by another remote round.
    #[allow(
        clippy::too_many_lines,
        reason = "one intermediate round's whole settlement contract stays in one place"
    )]
    async fn settle_mrtr_round(
        &self,
        remote_name: &str,
        identity: &McpCallIdentity<'_>,
        input_required: &rmcp::model::InputRequiredResult,
        context: &ToolExecutionContext<'_>,
        started: Instant,
    ) -> MrtrRoundSettlement {
        use crate::runtime::interaction::{
            InteractionFailure, InteractionOutcome, InteractionResponse,
        };

        // Only the test-only continuation-dispatch barrier reads the remote
        // name; the production path identifies the round by its context.
        #[cfg(not(test))]
        let _ = remote_name;

        let unsupported = |diagnostic: &str| {
            MrtrRoundSettlement::Terminal(failed_mcp(diagnostic, context, started))
        };
        // Cancellation observed before publication: no interaction is created
        // at all, so there is no pending prompt to settle and no waiter to
        // race.
        if context.cancellation.is_cancelled() {
            return MrtrRoundSettlement::Terminal(mcp_empty_terminal(
                ToolExecutionStatus::Cancelled {
                    reason: context.cancellation.reason(),
                    phase: crate::tools::types::ToolCancellationPhase::DuringExecution,
                },
                context,
                started,
            ));
        }
        if let Err(error) = mrtr::validate_request_state(input_required.request_state.as_deref()) {
            return unsupported(&error.diagnostic);
        }
        // Whole-payload validation before anything is published: a mixed
        // supported/unsupported request set fails here, never half-way
        // through a prompt.
        let plan = match mrtr::plan_round(input_required.input_requests.as_ref()) {
            Ok(plan) => plan,
            Err(error) => return unsupported(&error.diagnostic),
        };
        let Some(plan) = plan else {
            // A pure `requestState` continuation: the server asked for no
            // human input, so the next round carries only the opaque state.
            let continuation = mrtr::McpContinuation {
                request_state: input_required.request_state.clone(),
                input_responses: None,
            };
            if continuation.is_empty() {
                return unsupported(
                    "the MCP server returned an input_required result carrying neither input \
                     requests nor request state",
                );
            }
            return MrtrRoundSettlement::Continue(continuation);
        };
        // The one runtime-owned human-interaction authority. It is the same
        // crate-private capability the native `ask_user` tool consumes; the
        // MCP adapter never sees an `InteractionCoordinator`, never triggers
        // cancellation, and cannot create any other kind of interaction.
        let Some(requester) = context.questionnaire_requester() else {
            return unsupported(
                "the MCP server asked for user input, but this execution has no runtime-owned \
                 interaction authority: rustX advertises MCP elicitation only for invocations \
                 that can settle it",
            );
        };
        let facts = crate::runtime::interaction::QuestionnaireFacts {
            invocation_id: identity.invocation_id.clone(),
            // The canonical, provider-independent identity of the MCP tool
            // that asked. It is built from the registry-resolved invocation
            // and this runtime's own server identity — never from an rmcp
            // value and never from a display string — so every Runtime Client
            // can tell the human exactly which MCP server is asking.
            requester: identity.requester(&self.server_id),
            // The requester stamps the owning turn it was bound to; the
            // adapter has no turn of its own, exactly as `ask_user` does not.
            questionnaire: plan.questionnaire().clone(),
        };
        let response = match requester.request_questionnaire(facts).await {
            Ok(InteractionOutcome::Responded {
                response: InteractionResponse::Questionnaire { response },
            }) => response,
            Ok(InteractionOutcome::Responded { response: _ }) => {
                return unsupported(
                    "the MCP elicitation interaction settled with a response of another kind",
                );
            }
            Ok(InteractionOutcome::Cancelled { reason }) => {
                return MrtrRoundSettlement::Terminal(mcp_empty_terminal(
                    ToolExecutionStatus::Cancelled {
                        reason,
                        phase: crate::tools::types::ToolCancellationPhase::DuringExecution,
                    },
                    context,
                    started,
                ));
            }
            // The deadline expired while the questionnaire was pending, so
            // no remote request was in flight: the execution is provably
            // stopped rather than externally ambiguous.
            Ok(InteractionOutcome::DeadlineExpired { kind: _ }) => {
                return MrtrRoundSettlement::Terminal(mcp_empty_terminal(
                    ToolExecutionStatus::TimedOut,
                    context,
                    started,
                ));
            }
            // The existing coordinator contract, unchanged: provider absence
            // is never reported as a human decline, and a post-publication
            // control failure is a runtime failure of this invocation.
            Err(failure) => {
                let detail = if failure == InteractionFailure::Unavailable {
                    format!(
                        "the MCP server asked for user input and {failure}, so the call cannot \
                         continue"
                    )
                } else {
                    format!("the MCP elicitation interaction failed: {failure}")
                };
                return unsupported(&detail);
            }
        };
        let input_responses = match plan.responses(&response) {
            Ok(responses) => responses,
            Err(error) => return unsupported(&error),
        };
        // Test-only: the exact point between "the typed human response is
        // accepted" and "the continuation dispatch frontier is reached". A
        // regression parks here, wins or loses the cancellation race
        // deliberately, and asserts on the number of remote rounds.
        #[cfg(test)]
        test_sync::park_before_mrtr_continuation(remote_name, plan.request_count()).await;
        MrtrRoundSettlement::Continue(mrtr::McpContinuation {
            request_state: input_required.request_state.clone(),
            input_responses: Some(input_responses),
        })
    }

    /// The per-request client capabilities of **every** physical request one
    /// invocation sends: its `tools/call` rounds and the SEP-2663 task
    /// control requests of a task those rounds materialized.
    ///
    /// # Why capabilities are advertised per request, never per connection
    ///
    /// MCP `2026-07-28` carries client capabilities in every request's
    /// `_meta` (SEP-2575), which is exactly the granularity rustX's
    /// interaction ownership has: a foreground (or Workflow) invocation holds
    /// a runtime-owned Questionnaire capability, and a detached background
    /// invocation does not. Advertising elicitation connection-wide would tell
    /// a server that *every* call on that peer can be answered by a human,
    /// which is not true — and §4 of Issue #242 forbids exactly that.
    ///
    /// # Tasks and Elicitation are orthogonal, and are advertised separately
    ///
    /// Driving a remote MCP task is a *protocol* capability rustX always has
    /// on a modern peer: the poll loop, the bounded waits, the terminal-state
    /// projection, and cooperative cancellation are owned by the invocation
    /// itself and need no human. Answering an elicitation is an *authority*
    /// this particular invocation may or may not hold. So:
    ///
    /// ```text
    /// modern peer
    ///     -> _meta advertises extensions { io.modelcontextprotocol/tasks }
    /// modern peer + this invocation holds interaction authority
    ///     -> _meta additionally advertises elicitation { form }
    /// modern peer, no interaction authority
    ///     -> Tasks stay advertised; elicitation is absent, and an
    ///        input_required answer — from a round or from a task — is refused
    /// legacy peer
    ///     -> no request `_meta` capabilities at all
    /// ```
    ///
    /// A detached background invocation therefore drives
    /// `CreateTaskResult -> working -> completed` exactly like a foreground
    /// one, and fails honestly the moment such a task asks for a human.
    ///
    /// The connection-level handshake keeps `ClientCapabilities::default()`
    /// on both the legacy `initialize` path and the inline `server/discover`
    /// probe, so rustX never claims support for the legacy server-initiated
    /// `elicitation/create` callback it does not implement, and never
    /// declares Tasks to a peer whose requests could not carry the extension
    /// anyway. Legacy connections never receive request `_meta` capabilities
    /// at all: rmcp populates them only for `2026-07-28`+ peers, and this
    /// method adds nothing there.
    pub(super) fn invocation_request_options(
        &self,
        context: &ToolExecutionContext<'_>,
    ) -> PeerRequestOptions {
        if !uses_inline_lifecycle(&self.protocol_version) {
            return PeerRequestOptions::no_options();
        }
        let mut capabilities = ClientCapabilities::default();
        // SEP-2663. rustX implements the whole client half of the extension
        // — `tasks/get`, `tasks/update`, `tasks/cancel` — inside the owning
        // invocation, so declaring it is a contract rustX can keep.
        capabilities.extensions = Some(rmcp::model::ExtensionCapabilities::from([(
            rmcp::model::TASKS_EXTENSION_ID.to_owned(),
            rmcp::model::JsonObject::new(),
        )]));
        if context.questionnaire_requester().is_some() {
            // Form mode only. URL-mode elicitation directs a human to an
            // external site, which is not a bounded rustX Questionnaire.
            capabilities.elicitation = Some(
                rmcp::model::ElicitationCapability::new()
                    .with_form(rmcp::model::FormElicitationCapability::new()),
            );
        }
        let mut meta = rmcp::model::RequestMetaObject::new();
        meta.set_client_capabilities(capabilities);
        // rmcp fills protocol version, client info, and its own default
        // capabilities first and then extends with these explicit values, so
        // only the capabilities key is overridden.
        PeerRequestOptions::no_options().with_meta(meta)
    }

    /// Dispatches **one** physical MCP `tools/call` round and classifies its
    /// outcome (Issue #242).
    ///
    /// The physical request — its effect frontier, its ownership, and its
    /// cancellation arbitration — belongs to [`Self::dispatch_owned_request`],
    /// which every request of this invocation shares. What stays here is the
    /// `tools/call` *vocabulary*: how the round's request is built from the
    /// business arguments plus the MRTR continuation, and what each physical
    /// outcome means for a tool call.
    ///
    /// `continuation` carries the opaque protocol state of the previous
    /// round. On the first round it is empty and the request is byte-identical
    /// to the pre-#242 one. `started` is the **invocation's** start instant,
    /// shared by every round, so a settled result reports the whole
    /// invocation's duration rather than its last round's.
    async fn dispatch_one_mcp_round(
        &self,
        remote_name: &str,
        arguments: &serde_json::Map<String, serde_json::Value>,
        continuation: &mrtr::McpContinuation,
        context: &ToolExecutionContext<'_>,
        generation: u64,
        started: Instant,
    ) -> McpRoundOutcome {
        // The original business arguments are re-sent unchanged on every
        // round. MRTR continuation fields are additional *protocol* state and
        // are never merged into the model-issued arguments.
        let mut params =
            CallToolRequestParams::new(remote_name.to_owned()).with_arguments(arguments.clone());
        if let Some(request_state) = &continuation.request_state {
            // Echoed exactly as received: rustX never inspects, parses, or
            // re-encodes the opaque protocol state.
            params = params.with_request_state(request_state.clone());
        }
        if let Some(input_responses) = &continuation.input_responses {
            params = params.with_input_responses(input_responses.clone());
        }
        let outcome = self
            .dispatch_owned_request(
                ClientRequest::CallToolRequest(CallToolRequest::new(params)),
                McpRequestOwnership::tool_call(remote_name),
                context,
                generation,
                started,
            )
            .await;
        match outcome {
            McpPhysicalOutcome::RefusedByCancellation { reason } => {
                McpRoundOutcome::terminal(mcp_empty_terminal(
                    ToolExecutionStatus::Cancelled {
                        reason,
                        phase: crate::tools::types::ToolCancellationPhase::DuringExecution,
                    },
                    context,
                    started,
                ))
            }
            McpPhysicalOutcome::Refused { result, .. } => McpRoundOutcome::Terminal(*result),
            McpPhysicalOutcome::Observed {
                response,
                terminated_local_request,
                cancellation_won,
            } => {
                let outcome = self
                    .classify_post_frontier_response(
                        response,
                        context,
                        started,
                        generation,
                        terminated_local_request,
                    )
                    .await;
                if !cancellation_won {
                    return outcome;
                }
                match outcome {
                    // The peer answered this round with an intermediate MRTR
                    // result while cancellation intent had already won
                    // locally (Issue #242). Remote terminality *of this
                    // round* is proven — the server answered — and the
                    // invocation will not start another one, so the honest
                    // settlement is a proven cancellation, not an unknown
                    // outcome and not a failure.
                    McpRoundOutcome::InputRequired(_) => {
                        McpRoundOutcome::terminal(mcp_empty_terminal(
                            ToolExecutionStatus::Cancelled {
                                reason: context.cancellation.reason(),
                                phase: crate::tools::types::ToolCancellationPhase::DuringExecution,
                            },
                            context,
                            started,
                        ))
                    }
                    // A remote task was materialized by the very round
                    // cancellation just won against (Issue #243). Unlike an
                    // `input_required` answer, this one leaves *remote work
                    // running*: the server says the task exists. rustX will
                    // not poll it, asks the server cooperatively to stop it,
                    // and reports the outcome it genuinely cannot prove.
                    McpRoundOutcome::Task(created) => McpRoundOutcome::terminal(
                        self.abandon_remote_task(&created, context, started, generation)
                            .await,
                    ),
                    outcome @ McpRoundOutcome::Terminal(_) => outcome,
                }
            }
            McpPhysicalOutcome::CancelledPostFrontier { requested, .. } => {
                McpRoundOutcome::terminal(mcp_empty_terminal(
                    post_dispatch_cancellation_status(requested, &self.generation_tag(generation)),
                    context,
                    started,
                ))
            }
        }
    }

    /// Classifies one post-frontier `tools/call` outcome (Issue #205).
    ///
    /// Only a correlated remote response proves remote terminality; every
    /// other outcome leaves the external result unknown.
    ///
    /// `terminated_local_request` records whether *this call* terminated its
    /// own transport-level request as part of settling. When it did, a
    /// transport-send failure for this request is explained by that
    /// termination and is therefore not independent evidence that the
    /// connection generation itself is dead: the Streamable HTTP session
    /// survives an aborted request, and retiring the generation over
    /// rustX's own cancellation would force a needless reconnect on the
    /// next call.
    async fn classify_post_frontier_response(
        &self,
        response: McpResponseOutcome,
        context: &ToolExecutionContext<'_>,
        started: Instant,
        generation: u64,
        terminated_local_request: bool,
    ) -> McpRoundOutcome {
        let response = match response {
            McpResponseOutcome::Answered(answered) => *answered,
            McpResponseOutcome::ChannelEnded => {
                if !terminated_local_request {
                    self.note_transport_loss(
                        "the MCP response channel ended without a correlated response",
                    );
                }
                // The observation tee ends the stream on a confirmed
                // violation, so a violation surfaced mid-call lands here as
                // a transport close. It is a protocol failure, not an
                // anonymous disconnect — and the generation is poisoned.
                if let Some(diagnostic) = self.poisoned_protocol_violation().await {
                    return McpRoundOutcome::terminal(mcp_empty_terminal(
                        ToolExecutionStatus::OutcomeUnknown {
                            detail: bound_error(&diagnostic),
                        },
                        context,
                        started,
                    ));
                }
                // The transport closed after dispatch without a response:
                // the remote operation may have partially or fully completed.
                return McpRoundOutcome::terminal(mcp_empty_terminal(
                    ToolExecutionStatus::OutcomeUnknown {
                        detail: bound_error(&format!(
                            "MCP transport closed during tools/call without a response ({})",
                            self.generation_tag(generation)
                        )),
                    },
                    context,
                    started,
                ));
            }
        };
        match response {
            Ok(ServerResult::CallToolResult(result)) => {
                McpRoundOutcome::terminal(translate_result(result, context, started))
            }
            // The one intermediate state of the MRTR machine (Issue #242).
            // Whether it may be served at all is the driver's decision: this
            // classifier only reports what the peer answered.
            Ok(ServerResult::InputRequiredResult(result)) => {
                McpRoundOutcome::InputRequired(Box::new(result))
            }
            // The SEP-2663 Tasks extension: the server materialized this call
            // as a remote task (Issue #243). Like `InputRequiredResult` this
            // is a state, not a result; whether rustX may drive it at all is
            // the driver's decision, and this classifier only reports what
            // the peer answered.
            Ok(ServerResult::CreateTaskResult(result)) => McpRoundOutcome::Task(Box::new(result)),
            Ok(_) => McpRoundOutcome::terminal(failed_mcp(
                "unexpected MCP tools/call response",
                context,
                started,
            )),
            // A JSON-RPC error answering this request id is a correlated
            // remote response: the remote produced a terminal outcome and it
            // is a known failure, not an ambiguity.
            Err(rmcp::service::ServiceError::McpError(error)) => McpRoundOutcome::terminal(
                failed_mcp(&bound_error(&error.to_string()), context, started),
            ),
            Err(error) => {
                if is_transport_loss(&error) && !terminated_local_request {
                    self.note_transport_loss(&error.to_string());
                }
                // The observation tee ends the stream on a confirmed
                // violation, so a violation surfaced mid-call can land here.
                // It is a protocol failure, not an anonymous disconnect —
                // and the generation is poisoned.
                if let Some(diagnostic) = self.poisoned_protocol_violation().await {
                    return McpRoundOutcome::terminal(mcp_empty_terminal(
                        ToolExecutionStatus::OutcomeUnknown {
                            detail: bound_error(&diagnostic),
                        },
                        context,
                        started,
                    ));
                }
                McpRoundOutcome::terminal(mcp_empty_terminal(
                    ToolExecutionStatus::OutcomeUnknown {
                        detail: bound_error(&format!(
                            "the dispatched MCP tools/call produced no correlated remote \
                             response ({}): {error}",
                            self.generation_tag(generation)
                        )),
                    },
                    context,
                    started,
                ))
            }
        }
    }
}

/// The outcome of **one** physical MCP `tools/call` round (Issues #242, #243).
///
/// The distinction is the whole contract: `Terminal` settles the rustX
/// invocation, and neither of the other two does. `InputRequired` keeps the
/// invocation in the round loop; `Task` takes it out of the round loop for
/// good and into the remote task lifecycle. A protocol failure, a
/// cancellation, a deadline, and an unknown outcome are all already
/// `ToolExecutionResult` values by the time a round returns.
enum McpRoundOutcome {
    /// This round produced the invocation's one terminal result.
    Terminal(ToolExecutionResult),
    /// The server needs client input before the call can complete. This is
    /// intermediate execution-local protocol state, never a `ToolResult`.
    ///
    /// Boxed because `InputRequiredResult` carries a whole embedded request
    /// map and this value travels through the round loop by value.
    InputRequired(Box<rmcp::model::InputRequiredResult>),
    /// The server materialized this call as a remote task (SEP-2663). The
    /// invocation is **not** settled: its remote half moved into a task
    /// lifecycle, and no further `tools/call` will be sent.
    Task(Box<rmcp::model::CreateTaskResult>),
}

impl McpRoundOutcome {
    fn terminal(result: ToolExecutionResult) -> Self {
        Self::Terminal(result)
    }
}

/// The canonical rustX identity of the invocation one MCP call serves.
///
/// It is deliberately the **registry-resolved** identity, borrowed from the
/// admitted [`ToolInvocation`], rather than anything the adapter derives for
/// itself. Combined with this runtime's own `McpServerId` it produces the
/// provider-independent [`InteractionRequester`](crate::events::InteractionRequester)
/// that an MCP-originated Questionnaire carries all the way to the human.
pub(crate) struct McpCallIdentity<'a> {
    /// Caller-neutral invocation correlation.
    pub(super) invocation_id: &'a crate::tools::types::ToolInvocationId,
    /// The canonical registry-resolved tool identity.
    pub(super) tool_id: &'a crate::runtime::identity::ToolId,
    /// The safe model-facing tool name.
    pub(super) tool_name: &'a str,
}

impl McpCallIdentity<'_> {
    /// The canonical requester facts of an MCP-originated interaction.
    pub(super) fn requester(&self, server_id: &McpServerId) -> crate::events::InteractionRequester {
        crate::events::InteractionRequester {
            tool_id: self.tool_id.clone(),
            tool_name: self.tool_name.to_owned(),
            origin: crate::tools::types::ToolOrigin::Mcp {
                server_id: server_id.clone(),
            },
        }
    }
}

/// What one intermediate MRTR round settled to.
enum MrtrRoundSettlement {
    /// The invocation continues with this opaque protocol continuation.
    Continue(mrtr::McpContinuation),
    /// The invocation reached its one terminal result without another round.
    Terminal(ToolExecutionResult),
}
