//! The subagent child runtime driver (Issue #60): the `rustx
//! --subagent-child` internal mode.
//!
//! The child is a real rustX runtime — the same `ConversationRuntime`,
//! Agent Loop, Context Assembly, Tool Plane, and `ModelAdapter` as an
//! interactive session — composed headlessly from the typed
//! [`SubagentChildSpec`] that arrives over the control channel. The driver
//! itself is a thin bounded loop:
//!
//! ```text
//! fd 0 (inherited reliable control channel)
//!   -> Hello(spec)      version handshake; mismatch exits before compose
//!   -> dispatcher       the ONE owner of both raw transports from here on
//!   -> compose          the real runtime stack, deny-by-construction, and
//!                       cancellable owned work (Issue #145)
//!   -> Ready            composition and activation complete; live
//!                       inspection transport is optional
//!   -> Delegate(task)   the task enters through the child's ORDINARY
//!                       durable inbound path (UserSource::Agent(parent))
//!   -> observe          the attempt's canonical terminal event
//!   -> Result(candidate) exactly once, bounded
//!   -> drain + exit
//!
//! fd 1 (inherited disposable observation channel, Issue #178)
//!   -> Activity frames only, latest-value; its stall or loss is
//!      diagnostics-only and never delays or evidences control traffic
//! ```
//!
//! # One control dispatcher (Issue #145)
//!
//! Only the `Hello` version handshake reads the raw control `UnixStream`
//! directly: it must be decided before anything at all is composed.
//! Everything after it goes through [`ChildControlDispatcher`], the single
//! owner of both inherited transports, because the child now also creates
//! supervised process units that must offer their containment anchors to
//! the parent concurrently with `Delegate`/`Cancel`/`Result` traffic, and
//! because live activity rides its own disposable channel (Issue #178). No
//! Tool executor and no supervised-unit owner ever touches either stream.
//!
//! # Composition is cancellable owned work (Issue #145)
//!
//! External capability materialization can start an MCP process, negotiate a
//! protocol revision, and prepare the fingerprint-keyed uv environment of a
//! managed Python tool package (Issue #174). Composition therefore races the
//! attempt-derived cancellation and the control channel's EOF: the owner
//! cancels and awaits composition, then drains its retained physical owners.
//! The child never answers `Ready`, no semantic work begins, and the parent
//! settles the terminal from the child's physical outcome.
//!
//! # Message-bus invariant (child side)
//!
//! IPC never appends to the child's canonical history: the delegated task
//! becomes an ordinary durable inbound item through
//! [`ConversationRuntime::submit_sourced_inbound`], and the result travels
//! back as a **candidate** frame — terminal publication authority stays
//! with the parent-side settlement owner.
//!
//! # Parent-lifetime containment (child side)
//!
//! The control channel is the liveness authority: EOF means the parent is
//! gone, and the child then drains and exits without publishing a result
//! (the parent's recovery classifies the durable ownership as
//! interrupted). A `Cancel` frame requests the ordinary attempt
//! cancellation path; the child never exits on the frame alone — it
//! settles, reports, drains, and only then exits.
//!
//! # Cancellation is runtime-owned, not observation-driven
//!
//! `ParentFrame::Cancel` commits directly into the child
//! `ConversationRuntime`'s one-shot cancellation intent through
//! `ConversationRuntime::cancel_current_or_next_attempt`: a current
//! attempt's `AgentCancellation` is requested immediately, and a
//! still-unadmitted attempt starts already-cancelled when admission
//! consumes the intent. The `AttemptAdmitted` observation is **evidence,
//! never a control dependency** — the frame is never queued behind
//! observation delivery. The existing durable model-request-start frontier
//! (M9b) alone decides whether a model request may start.

use std::sync::Arc;

use chrono::Utc;
use futures_util::future::BoxFuture;

use crate::events::types::RuntimeEvent;
use crate::message::content::TextBlock;
#[cfg(test)]
use crate::message::types::MessageBlock;
use crate::message::types::{UserContentBlock, UserSource};
use crate::runtime::cancellation::CancellationSignal;
use crate::runtime::conversation_runtime::{InboundAdmissionError, ParentGuidanceSeal};
use crate::runtime::interaction::{
    InteractionAdmissionError, InteractionPublicationPermit, InteractionRef, InteractionRoute,
    InteractionRouteError, InteractionRouteEvent,
};
use crate::runtime::observation::{ConversationObservation, PendingObservations};
use crate::runtime::subagent::activity::SubagentObservationProjector;
use crate::runtime::subagent::ipc::{
    ActivityFrame, ChildFrame, ChildGuidanceOutcome, ChildGuidanceRefusal, ChildResultStatus,
    DiagnosticFrame, GuidanceResultFrame, ParentFrame, ReadyFrame, ResultFrame,
    SUBAGENT_IPC_VERSION, SubagentChildSpec, read_parent_frame, write_child_frame,
};
use crate::runtime::subagent::{
    MAX_RESULT_CONTENT_BYTES, bound_utf8, child_conversation_inspection_socket_path,
};

use super::composition::{ChildPreparation, LocalConversationCore, LocalRuntimeDependencies};
use super::dispatcher::{ChildControlDispatcher, ChildControlEvent, ChildControlHandle};
use super::live_inspection::{LiveConversationInspectionLease, LiveConversationInspectionServer};

/// The child-side adapter from the conversation-owned coordinator to the
/// parent's reliable control lane. It carries only route events; the parent
/// registry never receives the child's waiter, cancellation authority, or
/// settlement capability.
pub(crate) struct ChildInteractionRoute {
    handle: ChildControlHandle,
}

impl ChildInteractionRoute {
    pub(crate) fn new(handle: ChildControlHandle) -> Self {
        Self { handle }
    }

    fn frame(event: InteractionRouteEvent) -> ChildFrame {
        match event {
            InteractionRouteEvent::Requested(request) => ChildFrame::InteractionRequested(request),
            InteractionRouteEvent::Settled {
                interaction,
                outcome,
            } => ChildFrame::InteractionSettled {
                interaction,
                outcome,
            },
        }
    }
}

impl InteractionRoute for ChildInteractionRoute {
    fn admit_publication(
        &self,
        interaction: InteractionRef,
    ) -> BoxFuture<'static, Result<InteractionPublicationPermit, InteractionAdmissionError>> {
        self.handle.admit_interaction_publication(interaction)
    }

    fn publish(
        &self,
        event: InteractionRouteEvent,
    ) -> BoxFuture<'static, Result<(), InteractionRouteError>> {
        let handle = self.handle.clone();
        Box::pin(async move {
            handle
                .send_reliable_confirmed(Self::frame(event))
                .await
                .map_err(|_| InteractionRouteError::ControlLost)
        })
    }

    #[cfg(test)]
    fn try_publish(&self, event: InteractionRouteEvent) -> Result<(), InteractionRouteError> {
        self.handle
            .try_send_reliable(Self::frame(event))
            .map_err(|_| InteractionRouteError::ControlLost)
    }

    #[cfg(test)]
    fn try_admit_publication(
        &self,
        _interaction: InteractionRef,
    ) -> Result<InteractionPublicationPermit, InteractionAdmissionError> {
        Err(InteractionAdmissionError::ControlLost)
    }
}

/// The process entry point of the internal subagent-child mode.
///
/// Returns the process exit code: `0` for a settled run (including a
/// semantically failed or cancelled attempt — those are reported through
/// the `Result` frame, not the exit code), `2` for a startup failure
/// (already reported through `StartupError` when the channel allowed),
/// and `3` for a control-protocol violation of the parent.
pub async fn run_subagent_child() -> i32 {
    let mut control = match take_control_channel() {
        Ok(control) => control,
        Err(detail) => {
            eprintln!("subagent child: {detail}");
            return 2;
        }
    };
    // The version handshake is read from the raw stream, before the
    // dispatcher takes ownership of it: a peer that does not speak exactly
    // this protocol version must be refused before anything at all is
    // composed, including the dispatcher's own tasks.
    let spec = match read_parent_frame(&mut control).await {
        Ok(Some(ParentFrame::Hello(spec))) => {
            if spec.protocol_version != SUBAGENT_IPC_VERSION {
                let _ = write_child_frame(
                    &mut control,
                    &ChildFrame::StartupError(DiagnosticFrame {
                        message: format!(
                            "unsupported control protocol version {} (this build speaks \
                             {SUBAGENT_IPC_VERSION})",
                            spec.protocol_version
                        ),
                    }),
                )
                .await;
                return 2;
            }
            *spec
        }
        Ok(Some(_)) => {
            eprintln!("subagent child: the first control frame was not Hello");
            return 3;
        }
        Ok(None) => {
            // The parent died before the handshake: nothing to do.
            return 0;
        }
        Err(error) => {
            eprintln!("subagent child: {error}");
            return 3;
        }
    };
    // From here on there is exactly one owner of the raw transports. Every
    // other child-side owner — the semantic driver and every nested
    // supervised process unit — reaches the parent only through it.
    let observation = match take_observation_channel() {
        Ok(observation) => observation,
        Err(detail) => {
            eprintln!("subagent child: observation channel: {detail}");
            return 2;
        }
    };
    let mut dispatcher = ChildControlDispatcher::start(control, observation);
    let handle = dispatcher.handle();
    // The nested containment authority is installed BEFORE any composition
    // step can create a supervised process unit, so no unit can ever slip
    // past the anchor gate.
    if let Err(detail) =
        crate::runtime::nested_containment::install_authority(dispatcher.anchor_authority())
    {
        eprintln!("subagent child: {detail}");
        return 2;
    }

    let physical_lease =
        match crate::runtime::subagent::physical_recovery::ChildPhysicalLease::acquire(&spec) {
            Ok(lease) => lease,
            Err(error) => {
                let _ = handle
                    .send_reliable(ChildFrame::StartupError(DiagnosticFrame {
                        message: format!("physical incarnation lease: {error}"),
                    }))
                    .await;
                dispatcher.shutdown().await;
                return 2;
            }
        };
    let code = match Box::pin(run_child(&mut dispatcher, &handle, spec, &physical_lease)).await {
        Ok(()) => 0,
        // A startup failure was already reported through `StartupError`
        // by `run_child` when the channel allowed.
        Err(ChildExit::Startup(message)) => {
            let _ = handle
                .send_reliable(ChildFrame::StartupError(DiagnosticFrame {
                    message: bound_diagnostic(message),
                }))
                .await;
            2
        }
        Err(ChildExit::Protocol(message)) => {
            eprintln!("subagent child: {message}");
            3
        }
    };
    dispatcher.shutdown().await;
    code
}

/// The child's typed early exits.
#[derive(Debug)]
pub(crate) enum ChildExit {
    /// Composition failed; reportable through `StartupError`.
    Startup(String),
    /// The parent violated the bounded control protocol.
    Protocol(String),
}

/// The staged child run: compose, handshake, delegate, observe, report,
/// drain.
async fn run_child(
    dispatcher: &mut ChildControlDispatcher,
    handle: &ChildControlHandle,
    spec: SubagentChildSpec,
    physical_lease: &crate::runtime::subagent::physical_recovery::ChildPhysicalLease,
) -> Result<(), ChildExit> {
    let mut lifetime = ChildPhysicalLifetime::new(handle);
    let result = Box::pin(run_child_inner(dispatcher, handle, spec, &mut lifetime)).await;
    // Every normal return, including validation, composition and protocol
    // failure, reaches this one proof-producing epilogue. Aborting this owner
    // emits no proof. A failed drain retains uncertainty regardless of the
    // logical result or whether any process happens to remain observable.
    match lifetime.settle(handle.parent_lost()).await {
        Ok(()) => physical_lease.publish_quiescent().map_err(|error| {
            ChildExit::Protocol(format!("publish physical settlement receipt: {error}"))
        })?,
        Err(failures) => {
            eprintln!(
                "subagent child: physical settlement unproven: {}",
                failures.join("; ")
            );
        }
    }
    result
}

struct ChildPhysicalLifetime {
    preparation: ChildPreparation,
    runtime: Option<crate::runtime::conversation_runtime::ConversationRuntime>,
}

impl ChildPhysicalLifetime {
    fn new(handle: &ChildControlHandle) -> Self {
        Self {
            preparation: ChildPreparation::new(CancellationSignal::new(), handle.clone()),
            runtime: None,
        }
    }

    async fn settle(self, parent_lost: bool) -> Result<(), Vec<String>> {
        match self.runtime {
            Some(runtime) => runtime.settle_child_physical_lifetime(parent_lost).await,
            None => self.preparation.settle().await,
        }
    }
}

async fn run_child_inner(
    dispatcher: &mut ChildControlDispatcher,
    handle: &ChildControlHandle,
    spec: SubagentChildSpec,
    lifetime: &mut ChildPhysicalLifetime,
) -> Result<(), ChildExit> {
    if let Err(error) = spec.workspace_snapshot.validate() {
        return Err(ChildExit::Startup(format!(
            "the child workspace snapshot is invalid: {error}"
        )));
    }
    if spec.resolved.workspace_policy.is_isolated() != spec.workspace_snapshot.is_isolated() {
        return Err(ChildExit::Startup(
            "the child workspace policy and immutable workspace snapshot disagree".to_owned(),
        ));
    }
    let Some(core) = Box::pin(compose_cancellably(dispatcher, handle, &spec, lifetime)).await?
    else {
        // No semantic delegation began. Composition may already have owned
        // physical resources; the common lifetime epilogue must still drain
        // those before publishing proof for the parent's settlement.
        return Ok(());
    };
    let workflow_output = core.workflow_output();
    // The child is a real live Runtime Client owner. Its host receives the
    // primary projection queue, while the bounded parent activity projector
    // receives a separate fan-out queue; both are installed before the
    // activation cut, so live non-durable state belongs to the child's own
    // projection instead of being reconstructed from SQLite or tunneled as a
    // transcript through the parent observation lane.
    let (interactive, observations) = core
        .into_subagent_child_with_route(Arc::new(ChildInteractionRoute::new(handle.clone())))
        .map_err(|error| ChildExit::Startup(error.to_string()))?;
    let runtime = interactive.runtime().clone();
    let host = interactive.host().clone();
    let product = crate::runtime::local_storage::ProductRoot::existing(&spec.product_root)
        .map_err(|e| ChildExit::Startup(e.to_string()))?;
    let parent_runtime_root = product.root();
    // This lease is disposable process-routing state, not a durable
    // conversation fact. It lets an inspector distinguish a live child whose
    // optional endpoint failed from a child whose runtime is already gone.
    // If the lease itself cannot be created, execution still proceeds; the
    // failure is diagnosable in the child's private stderr log and through
    // the existing bounded Diagnostic frame.
    let live_lease = match LiveConversationInspectionLease::acquire(
        &spec.product_root,
        &spec.session_id,
        &spec.child_conversation_id,
    ) {
        Ok(lease) => Some(lease),
        Err(error) => {
            let message = format!("live inspection liveness lease unavailable: {error}");
            eprintln!("subagent child: {message}");
            let _ = handle
                .send_reliable(ChildFrame::Diagnostic(DiagnosticFrame {
                    message: bound_diagnostic(message),
                }))
                .await;
            None
        }
    };
    let live_server = match LiveConversationInspectionServer::bind(
        child_conversation_inspection_socket_path(parent_runtime_root, &spec.child_conversation_id),
        host,
    ) {
        Ok(server) => Some(server),
        Err(error) => {
            let message = format!("live inspection endpoint unavailable: {error}");
            eprintln!("subagent child: {message}");
            let _ = handle
                .send_reliable(ChildFrame::Diagnostic(DiagnosticFrame {
                    message: bound_diagnostic(message),
                }))
                .await;
            None
        }
    };

    let result = async {
        handle
            .send_reliable(ChildFrame::Ready(ReadyFrame {
                subagent_id: spec.subagent_id.clone(),
            }))
            .await
            .map_err(|error| ChildExit::Protocol(error.to_string()))?;
        mark_ready_sent_if_armed();
        serve_child_delegation(
            dispatcher,
            handle,
            spec.parent_agent_id,
            runtime.clone(),
            observations,
            workflow_output,
        )
        .await
    }
    .await;
    // A read-only inspector is a client of this process, not a reason to
    // extend the child's semantic lifetime. Once the child reports its
    // bounded result (or loses its parent), close the endpoint and remove
    // exactly its disposable socket path.
    if let Some(live_server) = live_server {
        live_server.shutdown().await;
    }
    drop(interactive);
    drop(live_lease);
    result
}

/// The child semantic loop after composition, activation, and the `Ready`
/// handshake: the start gate, the ordinary durable delegation inbound, the
/// terminal observation, the one bounded result candidate, and the drain.
///
/// This is the whole child-side conformance surface of Issue #138: the
/// attempt that runs here is an ordinary `ConversationRuntime` attempt with
/// the ordinary retry, deadline, tool-cancellation, publication, and
/// carryover semantics. The in-crate conformance suites drive this exact
/// function over a socket pair with a scripted model behind the same
/// runtime composition, so no child behavior is reimplemented in tests.
#[allow(clippy::too_many_lines)] // one bounded child control/delegation loop
pub(crate) async fn serve_child_delegation(
    dispatcher: &mut ChildControlDispatcher,
    handle: &ChildControlHandle,
    parent_agent_id: crate::runtime::identity::AgentId,
    runtime: crate::runtime::conversation_runtime::ConversationRuntime,
    observations: Arc<PendingObservations>,
    workflow_output: Option<Arc<crate::runtime::workflow::WorkflowOutputLatch>>,
) -> Result<(), ChildExit> {
    // The start gate: no semantic work before the delegation arrives.
    // Provider updates may already be queued because the root attachment can
    // detach while the child is waiting; apply them without treating them as
    // a delegation or an interaction settlement.
    let delegate = loop {
        match dispatcher.next_event().await {
            Some(ChildControlEvent::Delegate(delegate)) => break delegate,
            Some(ChildControlEvent::InteractionProviderAvailable { available }) => {
                runtime.set_interaction_provider_available(available);
            }
            Some(ChildControlEvent::InteractionRespond {
                response_id,
                interaction,
                response,
            }) => {
                send_interaction_response_result(
                    handle,
                    &runtime,
                    response_id,
                    interaction,
                    response,
                )
                .await?;
            }
            Some(ChildControlEvent::Guidance { guidance_id, .. }) => {
                // Ordering makes this unreachable in production — the driver
                // writes `Delegate` before it serves any command — but the
                // child never silently drops a guidance envelope: there is no
                // delegated conversation to steer yet, so it is refused.
                answer_guidance(
                    handle,
                    guidance_id,
                    ChildGuidanceOutcome::Refused(ChildGuidanceRefusal::NotDelegated),
                )
                .await?;
            }
            Some(ChildControlEvent::Cancel { .. }) | None => {
                // Cancelled (or orphaned) before any work began: drain and
                // exit. The parent settles the cancelled/interrupted terminal
                // itself from the physical outcome.
                let _ = runtime.shutdown().await;
                return Ok(());
            }
            Some(ChildControlEvent::SealGranted | ChildControlEvent::AdmissionReopened) => {
                return Err(ChildExit::Protocol("unexpected seal grant".to_owned()));
            }
            Some(ChildControlEvent::ProtocolViolation(message)) => {
                return Err(ChildExit::Protocol(message));
            }
        }
    };
    runtime.set_interaction_provider_available(delegate.interaction_provider_available);

    runtime.gate_child_turns();

    // The delegated task enters through the child's ordinary durable
    // inbound path. IPC transports the envelope; it never appends.
    let mut content = Vec::new();
    if let Some(context) = delegate.context {
        content.push(UserContentBlock::Text(TextBlock {
            text: format!("Context supplied by the delegating agent:\n{context}"),
        }));
    }
    content.push(UserContentBlock::Text(TextBlock {
        text: delegate.task,
    }));
    content.extend(
        delegate
            .attachments
            .into_iter()
            .map(UserContentBlock::UploadedFile),
    );
    if let Err(error) = runtime.submit_sourced_inbound(
        UserSource::Agent {
            agent_id: parent_agent_id.clone(),
        },
        content,
    ) {
        return report_and_drain(
            handle,
            &runtime,
            ResultFrame {
                status: ChildResultStatus::Failed,
                content: None,
                diagnostic: Some(bound_diagnostic(format!(
                    "the delegated task was refused by the child runtime: {error}"
                ))),
            },
        )
        .await;
    }

    handle
        .send_reliable(ChildFrame::DelegateAccepted)
        .await
        .map_err(|error| ChildExit::Protocol(error.to_string()))?;
    runtime.release_child_turn();

    // Observe the attempt to its canonical terminal event while serving
    // Cancel frames through the ordinary cancellation path.
    //
    // The live activity projection (Issue #178) taps the same drained
    // observation stream: every drained observation folds into the
    // child-owned projector, and each applied transition is published to
    // the dispatcher's disposable latest-value activity slot —
    // synchronous and non-blocking, so this loop never waits on
    // observation delivery and no separate forwarder task exists.
    //
    // # The terminal seal (Issue #193)
    //
    // A completed attempt is not by itself the terminal of a **steerable**
    // child: guidance the parent steered in may have been durably accepted
    // after this attempt's last safe boundary, and the ordinary coordinator
    // then admits the turn that observes it. Such a child therefore asks its
    // own conversation — under the one coordinator lock that owns durable
    // inbound acceptance — whether it may seal. `Open` means semantic work
    // is still owed and exactly one further ordinary attempt terminal
    // follows; `Sealed` means no accepted guidance remains unobserved and
    // none can be accepted afterwards; `DurabilityFailed` means the runtime
    // could not *prove* either, and the child then fails closed rather than
    // publishing an answer it cannot justify.
    //
    // This is the same one logical child, the same conversation, the same
    // process incarnation, the same registry record, and still exactly one
    // parent-side terminal settlement: the loop only refuses to report a
    // result that could not have observed an already-accepted steer.
    //
    // # Workflow-owned children never enter the seal
    //
    // The seal is steering-specific terminal machinery. A Workflow-owned
    // `AgentRun` (`workflow_output` latch present) is structurally not
    // messageable — it has no durable native Agent identity in the parent
    // registry, so `send_message` rejects it before any frame exists. No accepted
    // generic guidance can ever be pending in its conversation, and the
    // steering-specific seal must have **no semantic effect** on its
    // lifecycle or terminal result. The first attempt terminal is final,
    // regardless of outcome: the loop never consults the seal, so a seal
    // durable-probe failure can never convert a valid
    // Workflow output settlement into a failure. The one committed
    // `workflow_output` value remains the exactly-once terminal settlement
    // through the ordinary Workflow output path below.
    let mut cancelled = false;
    let mut observed_terminals: u64 = 0;
    let terminal = loop {
        let mut terminal = await_terminal(
            dispatcher,
            &runtime,
            &observations,
            handle,
            &parent_agent_id,
            &mut cancelled,
        )
        .await?;
        // Workflow completion is authoritative at its committed attempt terminal.
        // Only a continuable Agent absorbs later cancellation across turns.
        if cancelled && workflow_output.is_none() && !matches!(terminal, AttemptTerminal::Orphaned)
        {
            terminal = AttemptTerminal::Cancelled;
        }
        observed_terminals = observed_terminals.saturating_add(1);
        // Workflow-owned children settle on their first terminal. Orphans
        // cannot close admission with a parent that no longer exists.
        // Every other child first drains the parent's already-admitted FIFO
        // guidance into its durable inbox. Only natural completion may reopen
        // this activation: failure (including timeout/limits) and cancellation
        // are final. Any accepted, unobserved guidance remains durable input
        // owned by the Agent conversation for a later activation.
        if cancelled || workflow_output.is_some() || matches!(terminal, AttemptTerminal::Orphaned) {
            break terminal;
        }
        if !close_parent_admission(
            dispatcher,
            handle,
            &runtime,
            &parent_agent_id,
            &mut cancelled,
        )
        .await?
        {
            break AttemptTerminal::Cancelled;
        }
        if !matches!(terminal, AttemptTerminal::Completed(_)) {
            break terminal;
        }
        match runtime.seal_parent_guidance(observed_terminals).await {
            ParentGuidanceSeal::Sealed => break terminal,
            ParentGuidanceSeal::Open => {
                handle
                    .send_reliable(ChildFrame::SealOpen)
                    .await
                    .map_err(|error| ChildExit::Protocol(error.to_string()))?;
                if !await_parent_admission_boundary(
                    dispatcher,
                    handle,
                    &runtime,
                    &parent_agent_id,
                    true,
                    &mut cancelled,
                )
                .await?
                {
                    break AttemptTerminal::Cancelled;
                }
                runtime.release_child_turn();
            }
            // Fail closed: an unverifiable pending inbox is not an empty
            // one. The runtime already committed its absorbing
            // durability-failure fact; the completed attempt's answer is
            // deliberately discarded, because it may predate an accepted,
            // never-adopted steer. The child still reports exactly one
            // terminal, and it is a failure.
            ParentGuidanceSeal::DurabilityFailed { diagnostic } => {
                break AttemptTerminal::Failed(diagnostic);
            }
        }
    };
    let frame = match terminal {
        AttemptTerminal::Completed(attempt_id) => {
            let answer = workflow_output.as_ref().and_then(|latch| {
                latch
                    .committed_value()
                    .and_then(|value| serde_json::to_string(&value).ok())
            });
            let answer = if workflow_output.is_none() {
                final_answer(&runtime, &attempt_id)
            } else {
                answer
            };
            match answer {
                Some(answer) => ResultFrame {
                    status: ChildResultStatus::Succeeded,
                    content: Some(answer),
                    diagnostic: None,
                },
                None => ResultFrame {
                    status: ChildResultStatus::Failed,
                    content: None,
                    diagnostic: Some(if workflow_output.is_some() {
                        "the Workflow Agent completed without a valid workflow_output commit"
                            .to_owned()
                    } else {
                        "the attempt completed without a final answer".to_owned()
                    }),
                },
            }
        }
        AttemptTerminal::Cancelled => ResultFrame {
            status: ChildResultStatus::Cancelled,
            content: None,
            diagnostic: None,
        },
        AttemptTerminal::Failed(diagnostic) => ResultFrame {
            status: ChildResultStatus::Failed,
            content: None,
            diagnostic: Some(bound_diagnostic(diagnostic)),
        },
        AttemptTerminal::Orphaned => {
            // The reliable parent control path is gone. Return to run_child's
            // parent-loss drain: it abandons interaction waiters as ControlLost
            // before native cancellation, preserving requested audit facts and
            // proving physical containment without inventing a human outcome.
            return Ok(());
        }
    };
    report_and_drain(handle, &runtime, frame).await
}

/// The registry closes admission before the child can seal its inbox. Its grant
/// follows every message admitted before the close, on the same FIFO control lane.
async fn close_parent_admission(
    dispatcher: &mut ChildControlDispatcher,
    handle: &ChildControlHandle,
    runtime: &crate::runtime::conversation_runtime::ConversationRuntime,
    parent_agent_id: &crate::runtime::identity::AgentId,
    cancelled: &mut bool,
) -> Result<bool, ChildExit> {
    handle
        .send_reliable(ChildFrame::SealRequested)
        .await
        .map_err(|error| ChildExit::Protocol(error.to_string()))?;
    await_parent_admission_boundary(
        dispatcher,
        handle,
        runtime,
        parent_agent_id,
        false,
        cancelled,
    )
    .await
}

async fn await_parent_admission_boundary(
    dispatcher: &mut ChildControlDispatcher,
    handle: &ChildControlHandle,
    runtime: &crate::runtime::conversation_runtime::ConversationRuntime,
    parent_agent_id: &crate::runtime::identity::AgentId,
    reopening: bool,
    cancelled: &mut bool,
) -> Result<bool, ChildExit> {
    if *cancelled {
        return Ok(false);
    }
    loop {
        match dispatcher.next_event().await {
            Some(ChildControlEvent::SealGranted) if !reopening => return Ok(true),
            Some(ChildControlEvent::AdmissionReopened) if reopening => return Ok(true),
            Some(ChildControlEvent::SealGranted | ChildControlEvent::AdmissionReopened) => {
                return Err(ChildExit::Protocol(
                    "out-of-order admission boundary".into(),
                ));
            }
            Some(ChildControlEvent::Guidance {
                guidance_id,
                message,
                attachments,
            }) => {
                apply_parent_guidance(
                    handle,
                    runtime,
                    parent_agent_id,
                    guidance_id,
                    message,
                    attachments,
                )
                .await?;
            }
            Some(ChildControlEvent::InteractionProviderAvailable { available }) => {
                runtime.set_interaction_provider_available(available);
            }
            Some(ChildControlEvent::InteractionRespond {
                response_id,
                interaction,
                response,
            }) => {
                send_interaction_response_result(
                    handle,
                    runtime,
                    response_id,
                    interaction,
                    response,
                )
                .await?;
            }
            Some(ChildControlEvent::Cancel {
                reason: Some(reason),
            }) => {
                *cancelled = true;
                let _ = runtime.cancel_current_or_next_attempt(reason);
                // Cancellation is absorbing at BOTH admission boundaries.
                // It cannot be consumed by close and forgotten before reopen.
                return Ok(false);
            }
            Some(ChildControlEvent::ProtocolViolation(message)) => {
                return Err(ChildExit::Protocol(message));
            }
            Some(ChildControlEvent::Delegate(_) | ChildControlEvent::Cancel { reason: None })
            | None => {
                return Err(ChildExit::Protocol(
                    "parent lost while closing message admission".to_owned(),
                ));
            }
        }
    }
}

/// Composes the child runtime as **cancellable owned work** (Issue #145).
///
/// External capability materialization can take materially longer than the
/// old base-only startup — an MCP process start plus protocol negotiation, a
/// fingerprint-keyed uv environment build of a managed Python tool package
/// (Issue #174). Three things therefore race here, and the composition
/// owner is cancelled and awaited when either control authority wins:
///
/// ```text
/// Cancel from the parent   the spawn attempt no longer wants this child
/// parent control EOF       the parent process is gone
/// composition completes    the child may answer Ready
/// ```
///
/// `Ok(None)` means semantic preparation was cancelled before Ready. Any
/// physical owner created while composing remains in the lifetime epilogue;
/// only its explicit drain permits a physical receipt.
async fn compose_cancellably(
    dispatcher: &mut ChildControlDispatcher,
    handle: &ChildControlHandle,
    spec: &SubagentChildSpec,
    lifetime: &mut ChildPhysicalLifetime,
) -> Result<Option<LocalConversationCore>, ChildExit> {
    let preparation = &lifetime.preparation;
    let cancellation = preparation.cancellation();
    let dependencies = LocalRuntimeDependencies::default();
    let composition =
        LocalConversationCore::compose_subagent_child(spec, &dependencies, preparation);
    let mut composition = std::pin::pin!(composition);
    let mut events_open = true;
    let mut protocol_error = None;
    let composed = loop {
        tokio::select! {
            event = dispatcher.next_event(), if events_open => {
                let violation = match event {
                    Some(ChildControlEvent::Cancel { .. }) => {
                        cancellation.cancel();
                        None
                    }
                    Some(ChildControlEvent::InteractionProviderAvailable { .. }) => None,
                    None => {
                        events_open = false;
                        cancellation.cancel();
                        None
                    }
                    Some(ChildControlEvent::ProtocolViolation(message)) => Some(message),
                    Some(_) => Some("control frame arrived before the child answered Ready".into()),
                };
                if let Some(message) = violation {
                    // A protocol failure ends semantic admission, not physical
                    // ownership. Await the cancelled composition so its owner
                    // is available to the common settlement epilogue.
                    protocol_error = Some(message);
                    events_open = false;
                    cancellation.cancel();
                }
            },
            composed = &mut composition => break composed,
        }
    };
    if let Ok(core) = &composed {
        // Retain before any conversion/route bind can consume the core and
        // fail. From this cut onward the runtime owns the physical drain.
        lifetime.runtime = Some(core.runtime().clone());
    }
    if let Some(message) = protocol_error {
        return Err(ChildExit::Protocol(message));
    }
    match composed {
        Ok(core) => {
            // The final stretch of composition after the last guarded step
            // is not itself cancellation-checked, so a settlement authority
            // can win the race against the composition's completion. Once
            // pre-commit cancellation has won, `Ready` is impossible:
            // settle the composed runtime — its physical capability
            // runtimes included — and report the settled preparation.
            if cancellation.is_cancelled() || handle.parent_lost() {
                drop(core);
                Ok(None)
            } else {
                Ok(Some(core))
            }
        }
        Err(error) => {
            if cancellation.is_cancelled() || handle.parent_lost() {
                // A settled preparation is not a startup failure: the child
                // simply never became owned.
                Ok(None)
            } else {
                Err(ChildExit::Startup(format!("{error:?}")))
            }
        }
    }
}

/// Sends the one terminal result candidate and drains the runtime.
async fn report_and_drain(
    handle: &ChildControlHandle,
    runtime: &crate::runtime::conversation_runtime::ConversationRuntime,
    frame: ResultFrame,
) -> Result<(), ChildExit> {
    handle
        .send_reliable(ChildFrame::Result(frame))
        .await
        .map_err(|error| ChildExit::Protocol(error.to_string()))?;
    let _ = runtime.shutdown().await;
    Ok(())
}

/// Enters one parent-authored guidance envelope into the child's **ordinary**
/// durable inbound path and answers the parent with the child conversation's
/// authoritative decision (Issue #193).
///
/// This is the whole child-side semantics of steering, and it is deliberately
/// the same path the delegation itself took: the same coordinator lock, the
/// same durable acceptance linearization point, the same inbound sequence
/// domain, and the same ordinary Agent Loop safe-boundary adoption. Nothing
/// here interrupts the in-flight provider request, the partial generation, or
/// the executing tool call, and nothing re-authors the child's frozen launch
/// authority.
async fn apply_parent_guidance(
    handle: &ChildControlHandle,
    runtime: &crate::runtime::conversation_runtime::ConversationRuntime,
    parent_agent_id: &crate::runtime::identity::AgentId,
    guidance_id: u64,
    message: String,
    attachments: Vec<crate::message::content::UploadedFileRef>,
) -> Result<(), ChildExit> {
    let outcome = match runtime.submit_parent_guidance(
        UserSource::Agent {
            agent_id: parent_agent_id.clone(),
        },
        std::iter::once(UserContentBlock::Text(TextBlock { text: message }))
            .chain(attachments.into_iter().map(UserContentBlock::UploadedFile))
            .collect(),
    ) {
        Ok(_) => ChildGuidanceOutcome::Accepted,
        Err(InboundAdmissionError::GuidanceSealed) => {
            ChildGuidanceOutcome::Refused(ChildGuidanceRefusal::Settled)
        }
        Err(error) => ChildGuidanceOutcome::Refused(ChildGuidanceRefusal::Refused {
            detail: bound_diagnostic(error.to_string()),
        }),
    };
    #[cfg(test)]
    if matches!(outcome, ChildGuidanceOutcome::Accepted)
        && GUIDANCE_ACK_LOSS.try_with(|lose| *lose).unwrap_or(false)
    {
        return Err(ChildExit::Protocol(
            "injected control loss after durable guidance acceptance".into(),
        ));
    }
    answer_guidance(handle, guidance_id, outcome).await
}

/// Answers exactly one parent-authored guidance envelope over the reliable
/// control lane (Issue #193).
///
/// The parent owns admission; this child answer proves canonical durability
/// before the parent's `send_message` acknowledgement completes.
/// Every envelope receives exactly one answer; an envelope the child can no
/// longer serve is refused by the driver task dropping its waiter, never by
/// silence that the parent could mistake for acceptance.
async fn answer_guidance(
    handle: &ChildControlHandle,
    guidance_id: u64,
    outcome: ChildGuidanceOutcome,
) -> Result<(), ChildExit> {
    handle
        .send_reliable(ChildFrame::GuidanceResult(GuidanceResultFrame {
            guidance_id,
            outcome,
        }))
        .await
        .map_err(|error| ChildExit::Protocol(error.to_string()))
}

/// Applies one root-routed response at the child coordinator and returns the
/// coordinator's result over the same reliable control lane. The response is
/// never converted into a parent interaction or a child-parent transcript
/// message.
async fn send_interaction_response_result(
    handle: &ChildControlHandle,
    runtime: &crate::runtime::conversation_runtime::ConversationRuntime,
    response_id: u64,
    interaction: crate::runtime::interaction::InteractionRef,
    response: crate::runtime::interaction::InteractionControl,
) -> Result<(), ChildExit> {
    let result = runtime.control_interaction(&interaction, response).await;
    handle
        .send_reliable(ChildFrame::InteractionResponseResult(
            crate::runtime::subagent::ipc::InteractionResponseResultFrame {
                response_id,
                interaction,
                result,
            },
        ))
        .await
        .map_err(|error| ChildExit::Protocol(error.to_string()))
}

/// The canonical terminal and identity of one child attempt.
enum AttemptTerminal {
    /// `AttemptCompleted`.
    Completed(crate::runtime::identity::AttemptId),
    /// `AttemptCancelled`.
    Cancelled,
    /// `AttemptFailed`, `AttemptTimedOut`, or `AttemptLimitExceeded`, with
    /// the bounded diagnostic.
    Failed(String),
    /// The parent died mid-attempt (control channel EOF).
    Orphaned,
}

/// Drives the attempt to its terminal event, serving cancellation through
/// the ordinary runtime path and folding every drained observation into the
/// live activity projection (Issue #178). Applied transitions are published
/// through the dispatcher's disposable activity lane: synchronous and
/// non-blocking, so the agent loop never waits on observation delivery.
async fn await_terminal(
    dispatcher: &mut ChildControlDispatcher,
    runtime: &crate::runtime::conversation_runtime::ConversationRuntime,
    observations: &Arc<PendingObservations>,
    handle: &ChildControlHandle,
    parent_agent_id: &crate::runtime::identity::AgentId,
    cancelled: &mut bool,
) -> Result<AttemptTerminal, ChildExit> {
    await_terminal_inner(
        dispatcher,
        runtime,
        observations,
        handle,
        parent_agent_id,
        cancelled,
        |_| {},
    )
    .await
}

#[cfg(test)]
async fn await_terminal_with_probe(
    dispatcher: &mut ChildControlDispatcher,
    runtime: &crate::runtime::conversation_runtime::ConversationRuntime,
    observations: &Arc<PendingObservations>,
    handle: &ChildControlHandle,
    cancellation_before_admission: Arc<tokio::sync::Notify>,
    cancellation_after_admission: Arc<tokio::sync::Notify>,
) -> Result<AttemptTerminal, ChildExit> {
    await_terminal_inner(
        dispatcher,
        runtime,
        observations,
        handle,
        &crate::runtime::identity::AgentId::new("agent-parent"),
        &mut false,
        move |delivered| {
            if delivered {
                cancellation_after_admission.notify_one();
            } else {
                cancellation_before_admission.notify_one();
            }
        },
    )
    .await
}

#[allow(clippy::too_many_lines)] // one bounded control/observation select loop
async fn await_terminal_inner<F>(
    dispatcher: &mut ChildControlDispatcher,
    runtime: &crate::runtime::conversation_runtime::ConversationRuntime,
    observations: &Arc<PendingObservations>,
    handle: &ChildControlHandle,
    parent_agent_id: &crate::runtime::identity::AgentId,
    cancelled: &mut bool,
    on_cancellation: F,
) -> Result<AttemptTerminal, ChildExit>
where
    F: Fn(bool) + Send + Sync + 'static,
{
    // The child-owned live projector. Only applied transitions are
    // published, and the publication is a `watch` overwrite in place
    // (latest-value coalescing) that never waits on the consumer.
    let mut projector = SubagentObservationProjector::default();
    loop {
        tokio::select! {
            biased;
            () = handle.parent_lost_signal() => {
                // A reliable route owner marks the same parent-liveness
                // authority as the reader's EOF path. The child must not
                // report a healthy terminal or let a stale observation wake
                // it into another model turn after that boundary.
                return Ok(AttemptTerminal::Orphaned);
            }
            event = dispatcher.next_event() => {
                match event {
                    Some(ChildControlEvent::Cancel { reason }) => {
                        // The cancellation commits directly into the
                        // runtime-owned one-shot intent under the
                        // coordinator lock: a current attempt is cancelled
                        // immediately through its AgentCancellation, and a
                        // still-unadmitted attempt starts already-cancelled
                        // when the next admission consumes the intent.
                        // `delivered` reports whether a current attempt
                        // existed at this instant (test evidence); the
                        // pre-admission path arms the runtime intent.
                        // `AttemptAdmitted` observation is not part of this
                        // control path — the frame is never queued behind
                        // observation delivery.
                        let Some(reason) = reason else {
                            return Err(ChildExit::Protocol(
                                "a semantic cancellation arrived without a reason".to_owned(),
                            ));
                        };
                        *cancelled = true;
                        let delivered = runtime.cancel_current_or_next_attempt(reason).is_some();
                        #[cfg(test)]
                        let _ = CANCELLATION_OBSERVED.try_with(|signal| signal.notify_one());
                        on_cancellation(delivered);
                        // The frame is a request, not a terminal fact: the
                        // canonical AttemptCancelled settles the attempt.
                    }
                    Some(ChildControlEvent::Delegate(_)) => {
                        return Err(ChildExit::Protocol(
                            "a second delegation arrived during the attempt".to_owned(),
                        ));
                    }
                    Some(ChildControlEvent::Guidance {
                        guidance_id,
                        message,
                        attachments,
                    }) => {
                        apply_parent_guidance(
                            handle,
                            runtime,
                            parent_agent_id,
                            guidance_id,
                            message,
                            attachments,
                        )
                        .await?;
                    }
                    Some(ChildControlEvent::InteractionProviderAvailable { available }) => {
                        runtime.set_interaction_provider_available(available);
                    }
                    Some(ChildControlEvent::InteractionRespond {
                        response_id,
                        interaction,
                        response,
                    }) => {
                        send_interaction_response_result(
                            handle,
                            runtime,
                            response_id,
                            interaction,
                            response,
                        )
                        .await?;
                    }
                    Some(ChildControlEvent::SealGranted | ChildControlEvent::AdmissionReopened) => {
                return Err(ChildExit::Protocol("unexpected seal grant".to_owned()));
            }
            Some(ChildControlEvent::ProtocolViolation(message)) => {
                        return Err(ChildExit::Protocol(message));
                    }
                    None => return Ok(AttemptTerminal::Orphaned),
                }
            }
            () = observations.wait() => {
                for observation in observations.drain() {
                    // The activity tap runs first so even the terminal
                    // event's own transition is projected before this loop
                    // returns. The publication is synchronous and
                    // non-blocking (a `watch` overwrite): it can never
                    // disturb the attempt.
                    if projector.fold(&observation, Utc::now()) {
                        handle.publish_activity(ActivityFrame {
                            observation: projector.observation().clone(),
                        });
                    }
                    match observation {
                        ConversationObservation::Event { event, .. } => {
                            match event {
                                RuntimeEvent::AttemptCompleted { attempt_id, .. } => {
                                    return Ok(AttemptTerminal::Completed(attempt_id));
                                }
                                RuntimeEvent::AttemptCancelled { .. } => {
                                    return Ok(AttemptTerminal::Cancelled);
                                }
                                RuntimeEvent::AttemptFailed { error, .. } => {
                                    return Ok(AttemptTerminal::Failed(format!(
                                        "the child attempt failed: {error:?}"
                                    )));
                                }
                                RuntimeEvent::AttemptTimedOut { .. } => {
                                    return Ok(AttemptTerminal::Failed(
                                        "the child attempt exceeded its time budget"
                                            .to_owned(),
                                    ));
                                }
                                RuntimeEvent::AttemptLimitExceeded { limit, .. } => {
                                    return Ok(AttemptTerminal::Failed(format!(
                                        "the child attempt exceeded its {limit:?} limit"
                                    )));
                                }
                                _ => {}
                            }
                        }
                        ConversationObservation::Shutdown => {
                            return Ok(AttemptTerminal::Cancelled);
                        }
                        _ => {}
                    }
                }
            }
        }
    }
}

/// The bounded final assistant answer of the settled attempt.
fn final_answer(
    runtime: &crate::runtime::conversation_runtime::ConversationRuntime,
    attempt_id: &crate::runtime::identity::AttemptId,
) -> Option<String> {
    let assistant = runtime.durable_final_assistant(attempt_id)?;
    let answer = assistant
        .content
        .iter()
        .filter_map(|block| match block {
            crate::message::types::AssistantContentBlock::Text(text) => Some(text.text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("");
    (!answer.is_empty()).then(|| bound_utf8(answer, MAX_RESULT_CONTENT_BYTES))
}

/// Caps one diagnostic at the result-content bound.
fn bound_diagnostic(diagnostic: String) -> String {
    bound_utf8(diagnostic, MAX_RESULT_CONTENT_BYTES)
}

/// Test-only proof seam of the Issue #145 cancellation regressions: when
/// armed, the child writes a marker file at the instant it answers
/// `Ready`, so a parent-side test can prove *after the child's physical
/// exit* that no `Ready` was ever published. Inert in every other build;
/// the environment variable is read by nothing there.
#[cfg(test)]
fn mark_ready_sent_if_armed() {
    if let Some(path) = std::env::var_os(READY_MARKER_ENV) {
        std::fs::write(path, b"ready\n").expect("the Ready marker is writable");
    }
}

/// The inert non-test twin of [`mark_ready_sent_if_armed`].
#[cfg(not(test))]
const fn mark_ready_sent_if_armed() {}

/// The marker-file path of the Ready proof seam (test builds only).
#[cfg(test)]
const READY_MARKER_ENV: &str = "RUSTX_ISSUE145_READY_MARKER";

/// Takes over the inherited control channel on fd 0.
///
/// # Safety shim
///
/// This is the one explicitly allowed `unsafe` shim of the subagent
/// plane (see the `unsafe_code` policy in `Cargo.toml`): fd 0 is the
/// connected, blocking `UnixStream` endpoint the parent passed as the
/// child's standard input, and this call runs exactly once before any
/// other code touches fd 0.
#[allow(unsafe_code)]
fn take_control_channel() -> std::io::Result<tokio::net::UnixStream> {
    use std::os::unix::io::FromRawFd;
    // SAFETY: the parent passes the connected control-channel endpoint as
    // the child's fd 0 and this is the single takeover of it.
    let std_stream = unsafe { std::os::unix::net::UnixStream::from_raw_fd(0) };
    std_stream.set_nonblocking(true)?;
    tokio::net::UnixStream::from_std(std_stream)
}

/// Takes over the inherited observation channel on fd 1 (Issue #178).
///
/// fd 1 is the connected, blocking `UnixStream` endpoint the parent passed
/// as the child's standard output: the dedicated disposable transport for
/// `Activity` frames. It shares the fd-0 safety shim's contract — one
/// takeover, before anything else touches fd 1 — and nothing else in the
/// child may write to standard output: a stray write would corrupt
/// observation framing (a diagnostics-only failure), never the control
/// channel.
#[allow(unsafe_code)]
fn take_observation_channel() -> std::io::Result<tokio::net::UnixStream> {
    use std::os::unix::io::FromRawFd;
    // SAFETY: the parent passes the connected observation-channel endpoint
    // as the child's fd 1 and this is the single takeover of it.
    let std_stream = unsafe { std::os::unix::net::UnixStream::from_raw_fd(1) };
    std_stream.set_nonblocking(true)?;
    tokio::net::UnixStream::from_std(std_stream)
}

#[cfg(test)]
tokio::task_local! {
    static CANCELLATION_OBSERVED: Arc<tokio::sync::Notify>;
    pub(crate) static GUIDANCE_ACK_LOSS: bool;
}

#[cfg(test)]
pub(crate) mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::agent::execution::test_sync::StartBoundaryPause;
    use crate::capabilities::{CapabilityCoordinator, CapabilityCoordinatorConfig};
    use crate::context::{AgentStatusEngine, DefaultTokenEstimator, SessionContextPolicy};
    use crate::model::adapter::ModelAdapter;
    use crate::runtime::conversation_runtime::{
        ConversationContextConfig, ConversationRuntime, CoordinatorProbe, Gate,
        RuntimeConversationConfig,
    };
    use crate::runtime::identity::{AgentId, ConversationId};
    use crate::runtime::types::CancellationReason;
    use crate::scripted_suites::support::fake::{FakeModel, FakeStep};
    use crate::scripted_suites::support::model::scripted_session_model;
    use crate::tools::executor::ToolRegistry;
    use crate::tools::runtime::ConversationToolRuntime;

    async fn child_test_runtime(
        dir: &tempfile::TempDir,
        start_pause: Option<StartBoundaryPause>,
        admission_gate: Option<Arc<Gate>>,
        conversation_id: ConversationId,
        model: Arc<FakeModel>,
    ) -> ConversationRuntime {
        child_test_runtime_with_seal_gate(
            dir,
            start_pause,
            admission_gate,
            None,
            conversation_id,
            model,
        )
        .await
    }

    async fn child_test_runtime_with_seal_gate(
        dir: &tempfile::TempDir,
        start_pause: Option<StartBoundaryPause>,
        admission_gate: Option<Arc<Gate>>,
        parent_guidance_seal_gate: Option<Arc<Gate>>,
        conversation_id: ConversationId,
        model: Arc<FakeModel>,
    ) -> ConversationRuntime {
        child_test_runtime_full(
            dir,
            start_pause,
            admission_gate,
            parent_guidance_seal_gate,
            None,
            None,
            conversation_id,
            model,
            None,
            None,
        )
        .await
    }

    /// The child-runtime fixture with the Agent Loop's provider-arbitration
    /// barrier armed, so a test can hold an actually-started attempt live and
    /// **unable to settle** while it exercises the coordinator.
    async fn child_test_runtime_with_model_pause(
        dir: &tempfile::TempDir,
        model_arbitration_pause: crate::agent::execution::test_sync::ModelArbitrationPause,
        conversation_id: ConversationId,
        model: Arc<FakeModel>,
    ) -> ConversationRuntime {
        child_test_runtime_full(
            dir,
            None,
            None,
            None,
            None,
            Some(model_arbitration_pause),
            conversation_id,
            model,
            None,
            None,
        )
        .await
    }

    /// The full child-runtime fixture: the seal-gate build above plus an
    /// optional Workflow-output latch, so a Workflow-owned `AgentRun` child
    /// (terminal mode `WorkflowOutput`) can be composed exactly like
    /// production (`SubagentChildCore::workflow_output`).
    #[allow(clippy::too_many_arguments)] // one composition fixture
    pub(crate) async fn child_test_runtime_full(
        dir: &tempfile::TempDir,
        start_pause: Option<StartBoundaryPause>,
        admission_gate: Option<Arc<Gate>>,
        parent_guidance_seal_gate: Option<Arc<Gate>>,
        workflow_output: Option<Arc<crate::runtime::workflow::WorkflowOutputLatch>>,
        model_arbitration_pause: Option<crate::agent::execution::test_sync::ModelArbitrationPause>,
        conversation_id: ConversationId,
        model: Arc<FakeModel>,
        store: Option<Arc<crate::durable::SqliteConversationStore>>,
        tools: Option<ToolRegistry>,
    ) -> ConversationRuntime {
        let workspace = dir.path().join("workspace");
        std::fs::create_dir_all(&workspace).expect("workspace");
        let mut config = crate::tools::runtime::ConversationRuntimeConfig::new(
            &workspace,
            dir.path().join("artifacts"),
        )
        .with_extensions(crate::extensions::NativeAgentExtensions::with_agent_status(
            crate::context::AgentStatusConfig::default(),
        ));
        config.durable_binding =
            store.map(|store| crate::durable::ConversationStoreBinding::new(store));
        let tool_runtime = ConversationToolRuntime::from_config(conversation_id.clone(), config)
            .expect("tool runtime");
        let mut activation = crate::capabilities::AgentActivation::default();
        if let Some(tools) = &tools {
            activation.profile.tools.builtin = tools
                .definitions()
                .into_iter()
                .map(|tool| tool.name.clone())
                .collect();
        }
        let capability = CapabilityCoordinator::new(CapabilityCoordinatorConfig {
            source_demand: crate::capabilities::source::ToolSourceDemand::default(),
            conversation_id: conversation_id.clone(),
            workspace: tool_runtime.workspace().clone(),
            base_tool_registry: Arc::new(tools.unwrap_or_default()),
            extension_tools: tool_runtime.extension_tool_plane(),
            agent_activation: activation,
            skill_discovery: crate::skills::SkillDiscoveryConfig::default(),
            mcp_servers: std::collections::BTreeMap::new(),
            base_environment: tool_runtime.environment().clone(),
            environment_store_root: dir.path().join("environments"),
        })
        .expect("capability coordinator");
        let candidate = capability.prepare_candidate().await.expect("candidate");
        capability.commit(candidate).expect("capability commit");
        let adapter: Arc<dyn ModelAdapter> = model;
        ConversationRuntime::with_probe(
            RuntimeConversationConfig {
                explicit_model: true,
                agent_id: AgentId::new("agent-child"),
                model: scripted_session_model(adapter),
                approval_mode: crate::runtime::ApprovalMode::Policy,
                model_timeout_policy: crate::model::ModelTimeoutPolicy::default(),
                tool_deadline_policy: crate::tools::deadline::ToolExecutionDeadlinePolicy::default(
                ),
                context: ConversationContextConfig {
                    policy: SessionContextPolicy {
                        reserve_tokens: 0,
                        keep_recent_tokens: 0,
                        summary_output_cap: None,
                    },
                    estimator: Arc::new(DefaultTokenEstimator),
                    status_engine: Some(AgentStatusEngine::default()),
                },
                tool_runtime,
                resources: Arc::new(crate::runtime::RuntimeResourceSnapshot::new(
                    crate::runtime::RuntimeResourceRevision::new(1),
                    Vec::new(),
                    None,
                    crate::context::ContextAssembly::new(),
                    capability.current_snapshot(),
                )),
                resource_loader: Arc::new(crate::runtime::FilesystemRuntimeResourceLoader::new(
                    &workspace,
                )),
                capability,
                clock: None,
                initial_messages: Vec::new(),
                subagents: None,
                workflow_output: workflow_output.clone().map(
                    |latch| -> Arc<dyn crate::runtime::workflow::WorkflowOutputTerminal> { latch },
                ),
            },
            CoordinatorProbe {
                start_boundary_pause: start_pause,
                admission_gate,
                parent_guidance_seal_gate,
                model_arbitration_pause,
                ..CoordinatorProbe::default()
            },
        )
        .expect("child runtime")
    }

    /// Cancel before admission (Issue #60, Blocker B): the delegated
    /// inbound is durably accepted and the admission worker is parked
    /// before the coordinator lock; `ParentFrame::Cancel` commits the
    /// runtime-owned one-shot intent while no attempt exists; the released
    /// admission consumes the intent and the attempt starts
    /// already-cancelled. No observation delivery is involved: the
    /// `AttemptAdmitted` observation is provably still sitting unread in
    /// the queue when admission proceeds.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[allow(clippy::too_many_lines)]
    async fn cancel_before_attempt_admission_arms_the_one_shot_intent() {
        let dir = tempfile::tempdir().expect("temp root");
        let admission_gate = Arc::new(Gate::default());
        let model = Arc::new(FakeModel::new(Vec::new()));
        let conversation_id = ConversationId::new("conv_ef886774-3d40-707f-8581-4dc1d1d5ad46");
        let runtime = child_test_runtime(
            &dir,
            None,
            Some(admission_gate.clone()),
            conversation_id,
            model.clone(),
        )
        .await;
        let observations = Arc::new(PendingObservations::new());
        runtime
            .install_observation_bridge(Arc::clone(&observations))
            .expect("observation bridge");
        runtime.activate();
        admission_gate.arm();
        runtime
            .submit_sourced_inbound(
                UserSource::Agent {
                    agent_id: AgentId::new("agent-parent"),
                },
                vec![UserContentBlock::Text(TextBlock {
                    text: "delegated task".to_owned(),
                })],
            )
            .expect("Delegate enters ordinary child inbound");
        // The admission worker is parked before the coordinator lock: the
        // durable inbound is accepted but no attempt exists yet.
        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            tokio::task::spawn_blocking({
                let admission_gate = admission_gate.clone();
                move || admission_gate.wait_entered()
            }),
        )
        .await
        .expect("admission gate liveness")
        .expect("admission gate entered");

        let (mut parent_end, child_end) = tokio::net::UnixStream::pair().expect("control pair");
        let (_observation_parent_end, observation_child_end) =
            tokio::net::UnixStream::pair().expect("observation pair");
        crate::runtime::subagent::ipc::write_parent_frame(
            &mut parent_end,
            &ParentFrame::Cancel {
                reason: Some(CancellationReason::UserRequested),
            },
        )
        .await
        .expect("parent sends Cancel");
        let child_runtime = runtime.clone();
        let child_observations = Arc::clone(&observations);
        let cancellation_before_admission = Arc::new(tokio::sync::Notify::new());
        let cancellation_after_admission = Arc::new(tokio::sync::Notify::new());
        let before_probe = Arc::clone(&cancellation_before_admission);
        let after_probe = Arc::clone(&cancellation_after_admission);
        let waiter = tokio::spawn(async move {
            let mut child_end = ChildControlDispatcher::start(child_end, observation_child_end);
            let handle = child_end.handle();
            await_terminal_with_probe(
                &mut child_end,
                &child_runtime,
                &child_observations,
                &handle,
                before_probe,
                after_probe,
            )
            .await
        });
        // The child consumed Cancel and committed the runtime-owned intent
        // while the admission worker was still parked: no current attempt
        // existed, so the before-admission probe fired.
        cancellation_before_admission.notified().await;
        // `AttemptAdmitted` is deliberately NOT consumed here — observation
        // delivery is provably not part of the cancellation control path.
        admission_gate.release();

        let terminal = tokio::time::timeout(std::time::Duration::from_secs(10), waiter)
            .await
            .expect("child cancellation liveness")
            .expect("child waiter")
            .expect("child control loop");
        assert!(matches!(terminal, AttemptTerminal::Cancelled));
        assert!(
            model.requests().is_empty(),
            "no model request crossed cancellation: {:?}",
            model.requests()
        );
        assert!(
            !runtime
                .tool_runtime()
                .durable_store()
                .read_events(None, 64)
                .expect("events")
                .events
                .iter()
                .any(|event| matches!(event.event, RuntimeEvent::ModelRequestStarted { .. }))
        );
        let events = runtime
            .tool_runtime()
            .durable_store()
            .read_events(None, 64)
            .expect("events")
            .events;
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(
                    event.event,
                    RuntimeEvent::AttemptCancelled {
                        reason: CancellationReason::UserRequested,
                        ..
                    }
                ))
                .count(),
            1,
            "pre-admission cancellation keeps its typed reason in the child journal"
        );
        // The cancellation committed while the admission worker was still
        // parked (before-probe fired before `release`), so the one-shot
        // intent provably won the admission linearization. The shared
        // observation queue may have been drained by the child loop as
        // evidence — `AttemptAdmitted` delivery is not part of the control
        // path, which is exactly what the sequencing above proves.
        runtime.shutdown().await.expect("child runtime drains");
    }

    /// Cancel after admission, before request start (Issue #60, Blocker B):
    /// the attempt is parked at the existing M9 model-turn start boundary
    /// (before the cancellation-vs-start arbitration). `ParentFrame::Cancel`
    /// reaches the current attempt's `AgentCancellation` directly — no
    /// observation delivery is involved — and the M9b frontier resolves
    /// `CancelledBeforeStart`: zero `ModelRequestStarted`, zero provider
    /// requests.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[allow(clippy::too_many_lines)]
    async fn cancel_after_admission_before_request_start_wins_the_m9_frontier() {
        let dir = tempfile::tempdir().expect("temp root");
        let (pause, mut pre_start, _) = StartBoundaryPause::install(true, false);
        let model = Arc::new(FakeModel::new(Vec::new()));
        let conversation_id = ConversationId::new("conv_4e88e6f4-6f20-7093-8e67-022aac299e55");
        let runtime =
            child_test_runtime(&dir, Some(pause), None, conversation_id, model.clone()).await;
        let observations = Arc::new(PendingObservations::new());
        runtime
            .install_observation_bridge(Arc::clone(&observations))
            .expect("observation bridge");
        runtime.activate();
        runtime
            .submit_sourced_inbound(
                UserSource::Agent {
                    agent_id: AgentId::new("agent-parent"),
                },
                vec![UserContentBlock::Text(TextBlock {
                    text: "delegated task".to_owned(),
                })],
            )
            .expect("Delegate enters ordinary child inbound");
        // The attempt is admitted and parked at the M9 request-start
        // frontier, before the cancellation-vs-start arbitration.
        pre_start
            .as_mut()
            .expect("pre-start control")
            .await_park(1)
            .await;

        let (mut parent_end, child_end) = tokio::net::UnixStream::pair().expect("control pair");
        let (_observation_parent_end, observation_child_end) =
            tokio::net::UnixStream::pair().expect("observation pair");
        crate::runtime::subagent::ipc::write_parent_frame(
            &mut parent_end,
            &ParentFrame::Cancel {
                reason: Some(CancellationReason::UserRequested),
            },
        )
        .await
        .expect("parent sends Cancel");
        let child_runtime = runtime.clone();
        let child_observations = Arc::clone(&observations);
        let cancellation_after_admission = Arc::new(tokio::sync::Notify::new());
        let after_probe = Arc::clone(&cancellation_after_admission);
        let waiter = tokio::spawn(async move {
            let mut child_end = ChildControlDispatcher::start(child_end, observation_child_end);
            let handle = child_end.handle();
            await_terminal_with_probe(
                &mut child_end,
                &child_runtime,
                &child_observations,
                &handle,
                Arc::new(tokio::sync::Notify::new()),
                after_probe,
            )
            .await
        });
        // The child consumed Cancel and the runtime cancelled the current
        // attempt directly through its AgentCancellation (a current attempt
        // exists, so the after-admission probe fires). No observation was
        // consumed to make this happen.
        cancellation_after_admission.notified().await;
        pre_start.take().expect("pre-start control").release();

        let terminal = tokio::time::timeout(std::time::Duration::from_secs(10), waiter)
            .await
            .expect("child cancellation liveness")
            .expect("child waiter")
            .expect("child control loop");
        assert!(matches!(terminal, AttemptTerminal::Cancelled));
        assert!(
            model.requests().is_empty(),
            "zero provider requests crossed the M9 frontier: {:?}",
            model.requests()
        );
        assert!(
            !runtime
                .tool_runtime()
                .durable_store()
                .read_events(None, 64)
                .expect("events")
                .events
                .iter()
                .any(|event| matches!(event.event, RuntimeEvent::ModelRequestStarted { .. }))
        );
        let events = runtime
            .tool_runtime()
            .durable_store()
            .read_events(None, 64)
            .expect("events")
            .events;
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(
                    event.event,
                    RuntimeEvent::AttemptCancelled {
                        reason: CancellationReason::UserRequested,
                        ..
                    }
                ))
                .count(),
            1,
            "pre-start cancellation keeps its typed reason in the child journal"
        );
        runtime.shutdown().await.expect("child runtime drains");
    }

    /// Cancel after request start (Issue #60, Blocker B): the durable
    /// request-start frontier was crossed and the provider stream is parked
    /// awaiting cancellation (the parked watch is the production
    /// synchronization point). `ParentFrame::Cancel` cancels the in-flight
    /// request through the existing M9 semantics; exactly one request was
    /// started and no second model turn follows the cancellation
    /// settlement.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn cancel_after_request_start_cancels_the_in_flight_request() {
        let dir = tempfile::tempdir().expect("temp root");
        let model = Arc::new(FakeModel::new(vec![vec![FakeStep::ParkUntilCancelled]]));
        let conversation_id = ConversationId::new("conv_38e54f8b-1122-775c-882b-84b4d9eeea08");
        let runtime = child_test_runtime(&dir, None, None, conversation_id, model.clone()).await;
        let observations = Arc::new(PendingObservations::new());
        runtime
            .install_observation_bridge(Arc::clone(&observations))
            .expect("observation bridge");
        runtime.activate();
        runtime
            .submit_sourced_inbound(
                UserSource::Agent {
                    agent_id: AgentId::new("agent-parent"),
                },
                vec![UserContentBlock::Text(TextBlock {
                    text: "delegated task".to_owned(),
                })],
            )
            .expect("Delegate enters ordinary child inbound");
        // The request-start frontier was crossed: the provider stream is
        // parked awaiting cancellation.
        let mut parked = model.parked();
        parked
            .wait_for(|is_parked| *is_parked)
            .await
            .expect("provider parked watch");
        assert_eq!(model.requests().len(), 1, "exactly one request started");

        let (mut parent_end, child_end) = tokio::net::UnixStream::pair().expect("control pair");
        let (_observation_parent_end, observation_child_end) =
            tokio::net::UnixStream::pair().expect("observation pair");
        crate::runtime::subagent::ipc::write_parent_frame(
            &mut parent_end,
            &ParentFrame::Cancel {
                reason: Some(CancellationReason::UserRequested),
            },
        )
        .await
        .expect("parent sends Cancel");
        let child_runtime = runtime.clone();
        let child_observations = Arc::clone(&observations);
        let cancellation_after_admission = Arc::new(tokio::sync::Notify::new());
        let after_probe = Arc::clone(&cancellation_after_admission);
        let waiter = tokio::spawn(async move {
            let mut child_end = ChildControlDispatcher::start(child_end, observation_child_end);
            let handle = child_end.handle();
            await_terminal_with_probe(
                &mut child_end,
                &child_runtime,
                &child_observations,
                &handle,
                Arc::new(tokio::sync::Notify::new()),
                after_probe,
            )
            .await
        });
        // The child consumed Cancel and the runtime cancelled the in-flight
        // attempt directly.
        cancellation_after_admission.notified().await;

        let terminal = tokio::time::timeout(std::time::Duration::from_secs(10), waiter)
            .await
            .expect("child cancellation liveness")
            .expect("child waiter")
            .expect("child control loop");
        assert!(matches!(terminal, AttemptTerminal::Cancelled));
        assert_eq!(
            model.requests().len(),
            1,
            "one request total; cancellation never starts a second model turn"
        );
        runtime.shutdown().await.expect("child runtime drains");
    }

    fn preparation_fixture(
        dir: &tempfile::TempDir,
    ) -> (
        SubagentChildSpec,
        crate::runtime::local_storage::ProductRoot,
    ) {
        let workspace = dir.path().join("workspace");
        std::fs::create_dir_all(&workspace).expect("workspace");
        let spec = SubagentChildSpec {
            session_id: crate::runtime::identity::SessionId::new(
                "ses_01900000-0000-7000-8000-000000000001",
            ),

            protocol_version: SUBAGENT_IPC_VERSION,
            product_root: dir.path().to_path_buf(),
            subagent_id: crate::runtime::identity::SubagentId::new(
                "conv_4e7d4d77-5e91-762d-867c-f4b0134317e8",
            ),
            child_conversation_id: ConversationId::new("conv_4e7d4d77-5e91-762d-867c-f4b0134317e8"),
            child_agent_id: AgentId::new("agent-child"),
            parent_agent_id: AgentId::new("agent-parent"),
            resolved: crate::runtime::subagent::ResolvedSubagentSpec {
                environment: Vec::new(),
                generation: crate::runtime::identity::RuntimeResourceRevision::new(1),
                skill_roots: Vec::new(),

                selection: crate::runtime::agent_profile::FrozenAgentSelection::default(),
                agent: crate::runtime::subagent::SubagentName::parse("explore")
                    .expect("canonical name"),
                definition_digest: serde_json::from_value(serde_json::json!(
                    "sha256:0000000000000000000000000000000000000000000000000000000000000000"
                ))
                .expect("digest"),
                execution_deadline: None,
                workspace_policy: crate::runtime::workspace::WorkspacePolicy::SharedWorkspace,
                instructions: "frozen child instructions".to_owned(),
                model: crate::model::frozen::test_frozen_model_spec(
                    serde_json::from_value(serde_json::json!("local/model")).expect("model ref"),
                ),
                tools: Vec::new(),
                skills: Vec::new(),
                project_instructions: Vec::new(),
                materialization:
                    crate::runtime::subagent::resolver::ResolvedSubagentMaterialization::default(),
                extensions: crate::extensions::NativeAgentExtensionsDocument::default().resolve(),
            },
            approval_mode: crate::runtime::ApprovalMode::Policy,
            model_timeout_policy: crate::model::ModelTimeoutPolicy::default(),
            tool_deadline_policy: crate::tools::deadline::ToolExecutionDeadlinePolicy::default(),
            context: SessionContextPolicy {
                reserve_tokens: 0,
                keep_recent_tokens: 0,
                summary_output_cap: None,
            },
            workspace_snapshot: crate::runtime::workspace::WorkspaceSnapshot::shared(
                workspace.clone(),
            ),
            incarnation: "incarnation-test".to_owned(),
            terminal: crate::runtime::subagent::ipc::ChildTerminalMode::Normal,
        };
        // Initialize the supported local-root format before fabricating the
        // child allocation tree, so composition's own root admission accepts it.
        let product =
            crate::runtime::local_storage::ProductRoot::create(&spec.product_root).unwrap();
        let allocation = crate::runtime::subagent::child_conversation_store_path(
            product.root(),
            &spec.session_id,
            &spec.child_conversation_id,
        )
        .parent()
        .unwrap()
        .join(&spec.incarnation);
        std::fs::create_dir_all(&allocation).unwrap();
        (spec, product)
    }

    fn child_physical_lease(
        spec: &SubagentChildSpec,
    ) -> crate::runtime::subagent::physical_recovery::ChildPhysicalLease {
        use crate::runtime::subagent::physical_recovery::{
            ChildPhysicalLease, ParentPhysicalLease,
        };
        let product =
            crate::runtime::local_storage::ProductRoot::existing(&spec.product_root).unwrap();
        let parent = ParentPhysicalLease::reserve(
            &product,
            &spec.session_id,
            &spec.child_conversation_id,
            &spec.subagent_id,
        )
        .unwrap();
        // Duplicate the real pre-admission authority, then enter the same
        // Running transition as fd2 acquisition in a child process.
        ChildPhysicalLease::acquire_from_parent(spec, &parent).unwrap()
    }

    #[tokio::test]
    async fn composed_child_rollback_drains_without_semantic_activation() {
        let dir = tempfile::tempdir().unwrap();
        let (spec, _product) = preparation_fixture(&dir);
        let (_parent, child) = tokio::net::UnixStream::pair().unwrap();
        let (_observation_parent, observation_child) = tokio::net::UnixStream::pair().unwrap();
        let mut dispatcher = ChildControlDispatcher::start(child, observation_child);
        let handle = dispatcher.handle();
        let mut lifetime = ChildPhysicalLifetime::new(&handle);
        let core = compose_cancellably(&mut dispatcher, &handle, &spec, &mut lifetime)
            .await
            .unwrap()
            .unwrap();
        let runtime = core.runtime().clone();
        drop(core);
        assert!(!runtime.is_activated());
        lifetime.settle(false).await.unwrap();
        assert_eq!(
            runtime.lifecycle_state(),
            crate::runtime::types::ConversationLifecycleState::Quiescent
        );
        assert!(!runtime.has_current_attempt());
        assert!(runtime.durable_ledger().unwrap().is_empty());
        dispatcher.shutdown().await;
    }

    #[tokio::test]
    async fn composition_error_publishes_proof_through_the_physical_epilogue() {
        let dir = tempfile::tempdir().unwrap();
        let (mut spec, product) = preparation_fixture(&dir);
        // This is checked AFTER capability materialization and commit, before
        // constructing a ConversationRuntime. The retained preparation owner
        // must drain even though no runtime exists to perform shutdown.
        spec.terminal = crate::runtime::subagent::ipc::ChildTerminalMode::WorkflowOutput {
            output_schema: serde_json::json!({"type": "invalid-schema-type"}),
        };
        let activation = spec.subagent_id.clone();
        let conversation = spec.child_conversation_id.clone();
        let session = spec.session_id.clone();
        let lease = child_physical_lease(&spec);
        let (_parent, child) = tokio::net::UnixStream::pair().unwrap();
        let (_observation_parent, observation_child) = tokio::net::UnixStream::pair().unwrap();
        let mut dispatcher = ChildControlDispatcher::start(child, observation_child);
        let handle = dispatcher.handle();
        let result = run_child(&mut dispatcher, &handle, spec, &lease).await;
        assert!(
            matches!(&result, Err(ChildExit::Startup(message))
            if message.contains("output") && message.contains("schema")),
            "{result:?}"
        );
        dispatcher.shutdown().await;
        drop(lease);
        assert!(
            crate::runtime::subagent::physical_recovery::prove_after_release(
                &product,
                &session,
                &conversation,
                &activation,
            )
            .unwrap()
            .is_some()
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn protocol_failure_awaits_composition_before_publishing_physical_proof() {
        let dir = tempfile::tempdir().unwrap();
        let (spec, product) = preparation_fixture(&dir);
        let gate = crate::local_runtime::composition::arm_test_preparation_drain_gate(
            &spec.runtime_root().unwrap(),
        );
        let activation = spec.subagent_id.clone();
        let conversation = spec.child_conversation_id.clone();
        let session = spec.session_id.clone();
        let (mut parent, child) = tokio::net::UnixStream::pair().unwrap();
        let (_observation_parent, observation_child) = tokio::net::UnixStream::pair().unwrap();
        let child = tokio::spawn(async move {
            let lease = child_physical_lease(&spec);
            let mut dispatcher = ChildControlDispatcher::start(child, observation_child);
            let handle = dispatcher.handle();
            let result = run_child(&mut dispatcher, &handle, spec, &lease).await;
            dispatcher.shutdown().await;
            (result, lease)
        });
        gate.entered().await;
        crate::runtime::subagent::ipc::write_parent_frame(
            &mut parent,
            &ParentFrame::Delegate(crate::runtime::subagent::ipc::DelegationFrame {
                attachments: Vec::new(),
                task: "too early".into(),
                context: None,
                interaction_provider_available: false,
            }),
        )
        .await
        .unwrap();
        // The exact composition signal proves the protocol violation has been
        // consumed; the guarded owner still cannot complete before release.
        gate.cancellation().cancelled().await;
        assert!(!child.is_finished());
        let receipt = crate::runtime::subagent::child_conversation_store_path(
            product.root(),
            &session,
            &conversation,
        )
        .parent()
        .unwrap()
        .join("physical-settlement")
        .join(activation.as_str())
        .join("physical-settlement.json");
        let evidence: serde_json::Value =
            serde_json::from_slice(&std::fs::read(receipt).unwrap()).unwrap();
        assert_eq!(
            evidence["phase"], "running",
            "cancellation cannot publish proof before the held preparation drains"
        );
        gate.release();
        let (result, lease) = child.await.unwrap();
        assert!(matches!(result, Err(ChildExit::Protocol(_))));
        drop(lease);
        assert!(
            crate::runtime::subagent::physical_recovery::prove_after_release(
                &product,
                &session,
                &conversation,
                &activation,
            )
            .unwrap()
            .is_some()
        );
    }

    /// The Issue #145 local race, deterministically in-process (the e2e
    /// module covers the cross-process ordering; see
    /// `local_runtime::preparation_e2e`): a `Cancel` event sets
    /// the one preparation cancellation signal, and only THEN the guarded
    /// external-preparation step completes. The gate is deliberately
    /// biased to let the completed step win its internal race, so the
    /// composition's own settlement checks are what must hold: the
    /// composition must never be publishable as `Ready`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[allow(clippy::too_many_lines)] // complete child composition and ownership fixture
    async fn a_step_completing_after_cancellation_never_publishes_ready() {
        let dir = tempfile::tempdir().expect("temp root");
        let (spec, product) = preparation_fixture(&dir);
        let runtime_root = spec.runtime_root().unwrap();
        let gate = crate::local_runtime::composition::arm_test_preparation_gate(&runtime_root);

        let (parent, child) = tokio::net::UnixStream::pair().expect("control pair");
        let (_observation_parent, observation_child) =
            tokio::net::UnixStream::pair().expect("observation pair");
        let mut dispatcher = ChildControlDispatcher::start(child, observation_child);
        let handle = dispatcher.handle();
        let activation = spec.subagent_id.clone();
        let conversation = spec.child_conversation_id.clone();
        let session = spec.session_id.clone();
        let mut composed = tokio::spawn(async move {
            let lease = child_physical_lease(&spec);
            let outcome = Box::pin(run_child(&mut dispatcher, &handle, spec, &lease)).await;
            (outcome, dispatcher, lease)
        });

        // 1. The child is provably inside external preparation.
        tokio::select! {
            () = gate.entered() => {},
            result = &mut composed => panic!("composition exited before preparation gate: {:?}",
                result.map(|(outcome, _, _)| outcome)),
        }

        // 2. The parent's Cancel frame is written...
        let (mut parent_read, mut parent_write) = parent.into_split();
        crate::runtime::subagent::ipc::write_parent_frame(
            &mut parent_write,
            &ParentFrame::Cancel { reason: None },
        )
        .await
        .expect("the Cancel frame reaches the child");

        // 3. ...and the child provably consumed it: the exact cancellation
        //    signal the gated step runs under is now set.
        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            gate.cancellation().cancelled(),
        )
        .await
        .expect("liveness: the child consumed the Cancel event");

        // 4. Only NOW does the racy external step complete. The gate's
        //    release arm is biased ahead of its cancellation arm, so the
        //    step completes `Ok` — the exact shape of the dangerous race.
        gate.release();

        // 5. The composition is never publishable: it settles instead of
        //    becoming a runtime the driver would answer `Ready` for.
        let (outcome, dispatcher, lease) =
            tokio::time::timeout(std::time::Duration::from_secs(30), composed)
                .await
                .expect("liveness: the settled composition must complete")
                .expect("the composition task must not panic");
        outcome.expect("the settled composition is not a startup failure");
        // The real early-exit epilogue wrote this evidence, while the writer
        // remains excluded until its physical lease is released.
        assert!(
            crate::runtime::subagent::physical_recovery::prove(
                &product,
                &session,
                &conversation,
                &activation,
            )
            .unwrap()
            .is_none()
        );
        drop(lease);
        assert!(
            crate::runtime::subagent::physical_recovery::prove_after_release(
                &product,
                &session,
                &conversation,
                &activation,
            )
            .unwrap()
            .is_some()
        );
        // No child frame at all was written: not Ready, not anything. The
        // dispatcher's explicit shutdown makes the EOF deterministic.
        dispatcher.shutdown().await;
        let frame = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            crate::runtime::subagent::ipc::read_child_frame(&mut parent_read),
        )
        .await
        .expect("liveness: the wire closes with the dispatcher");
        assert_eq!(frame, Ok(None), "the settled child is silent on the wire");
    }

    // -----------------------------------------------------------------
    // The child conversation's terminal seal for parent-authored guidance
    // (Issue #193)
    // -----------------------------------------------------------------

    /// Scripts one plain answering model turn.
    fn answer(text: &str) -> Vec<FakeStep> {
        vec![
            FakeStep::Emit(crate::model::event::ModelEvent::Started),
            FakeStep::Emit(crate::model::event::ModelEvent::TextDelta {
                block_index: crate::message::types::ContentBlockIndex::new(0),
                text: text.to_owned(),
            }),
            FakeStep::Emit(crate::model::event::ModelEvent::Completed {
                finish_reason: crate::model::finish::ModelFinishReason::Stop,
                usage: None,
            }),
        ]
    }

    /// Wires one child runtime to the production `serve_child_delegation`
    /// loop over a real control socket pair, with the test holding the
    /// parent end.
    struct SealFixture {
        parent: tokio::net::UnixStream,
        serve: tokio::task::JoinHandle<Result<(), ChildExit>>,
    }

    fn serve_child(runtime: &ConversationRuntime) -> SealFixture {
        serve_child_with_output(runtime, None)
    }

    /// The full [`serve_child`] fixture: also hands the child driver the
    /// optional Workflow-output latch it would own in production for a
    /// Workflow `AgentRun` child (`SubagentChildCore::workflow_output`).
    fn serve_child_with_output(
        runtime: &ConversationRuntime,
        workflow_output: Option<Arc<crate::runtime::workflow::WorkflowOutputLatch>>,
    ) -> SealFixture {
        serve_child_with_reopen_gate(runtime, workflow_output, None)
    }

    fn serve_child_with_reopen_gate(
        runtime: &ConversationRuntime,
        workflow_output: Option<Arc<crate::runtime::workflow::WorkflowOutputLatch>>,
        reopen_gate: Option<Arc<Gate>>,
    ) -> SealFixture {
        let (parent, relay) = tokio::net::UnixStream::pair().expect("test parent pair");
        let (driver, child_end) = tokio::net::UnixStream::pair().expect("control pair");
        let (mut driver_read, driver_write) = driver.into_split();
        let driver_write = Arc::new(tokio::sync::Mutex::new(driver_write));
        let (mut relay_read, mut relay_write) = relay.into_split();
        let writer = Arc::clone(&driver_write);
        tokio::spawn(async move {
            use crate::runtime::subagent::ipc::{read_parent_frame, write_parent_frame};
            while let Ok(Some(frame)) = read_parent_frame(&mut relay_read).await {
                if write_parent_frame(&mut *writer.lock().await, &frame)
                    .await
                    .is_err()
                {
                    break;
                }
            }
        });
        tokio::spawn(async move {
            use crate::runtime::subagent::ipc::{
                read_child_frame, write_child_frame, write_parent_frame,
            };
            while let Ok(Some(frame)) = read_child_frame(&mut driver_read).await {
                // These tests isolate child-local durable sealing. Registry
                // tests separately prove arbitration before granting this frame.
                let result = if matches!(frame, ChildFrame::SealRequested) {
                    write_parent_frame(&mut *driver_write.lock().await, &ParentFrame::SealGranted)
                        .await
                } else if matches!(frame, ChildFrame::SealOpen) {
                    if let Some(gate) = &reopen_gate {
                        tokio::task::block_in_place(|| gate.enter());
                    }
                    write_parent_frame(
                        &mut *driver_write.lock().await,
                        &ParentFrame::AdmissionReopened,
                    )
                    .await
                } else {
                    write_child_frame(&mut relay_write, &frame).await
                };
                if result.is_err() {
                    break;
                }
            }
        });
        let (_observation_parent, observation_child) =
            tokio::net::UnixStream::pair().expect("observation pair");
        let observations = Arc::new(PendingObservations::new());
        runtime
            .install_observation_bridge(Arc::clone(&observations))
            .expect("observation bridge");
        // Match production composition: historical pending guidance cannot
        // admit a turn between activation and the Delegate handshake.
        runtime.gate_child_turns();
        runtime.activate();
        let child_runtime = runtime.clone();
        let serve = tokio::spawn(async move {
            let mut dispatcher = ChildControlDispatcher::start(child_end, observation_child);
            let handle = dispatcher.handle();
            let result = serve_child_delegation(
                &mut dispatcher,
                &handle,
                AgentId::new("agent-parent"),
                child_runtime,
                observations,
                workflow_output,
            )
            .await;
            dispatcher.shutdown().await;
            result
        });
        SealFixture { parent, serve }
    }

    async fn delegate(parent: &mut tokio::net::UnixStream, task: &str) {
        crate::runtime::subagent::ipc::write_parent_frame(
            parent,
            &ParentFrame::Delegate(crate::runtime::subagent::ipc::DelegationFrame {
                attachments: Vec::new(),
                task: task.to_owned(),
                context: None,
                interaction_provider_available: false,
            }),
        )
        .await
        .expect("parent delegates");
        assert!(matches!(
            crate::runtime::subagent::ipc::read_child_frame(parent)
                .await
                .unwrap(),
            Some(ChildFrame::DelegateAccepted)
        ));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn delegation_durable_acceptance_failure_never_acknowledges_input() {
        use crate::durable::ConversationStore;
        let dir = tempfile::tempdir().unwrap();
        let id = ConversationId::generate();
        let store =
            Arc::new(crate::durable::SqliteConversationStore::in_memory(id.clone()).unwrap());
        let model = Arc::new(FakeModel::new(vec![answer("must not run")]));
        let runtime = child_test_runtime_full(
            &dir,
            None,
            None,
            None,
            None,
            None,
            id,
            model.clone(),
            Some(store.clone()),
            None,
        )
        .await;
        let mut fixture = serve_child(&runtime);
        // Fault the exact durable acceptance transaction before any Delegate arrives.
        store.arm_fail_next_accept_commit();
        crate::runtime::subagent::ipc::write_parent_frame(
            &mut fixture.parent,
            &ParentFrame::Delegate(crate::runtime::subagent::ipc::DelegationFrame {
                attachments: Vec::new(),
                task: "input that cannot commit".into(),
                context: None,
                interaction_provider_available: false,
            }),
        )
        .await
        .unwrap();
        // The first reliable response must be failure, never DelegateAccepted.
        let result = read_result(&mut fixture.parent).await;
        assert_eq!(result.status, ChildResultStatus::Failed);
        assert!(
            result
                .diagnostic
                .unwrap()
                .contains("delegated task was refused")
        );
        assert!(store.load_pending().unwrap().is_empty());
        assert!(store.load_canonical().unwrap().is_empty());
        assert!(
            tokio::time::timeout(
                std::time::Duration::from_secs(20),
                crate::runtime::subagent::ipc::read_child_frame(&mut fixture.parent),
            )
            .await
            .expect("control close liveness")
            .unwrap()
            .is_none()
        );
        fixture.serve.await.unwrap().unwrap();
        assert!(model.requests().is_empty());
    }

    /// Receiver-contract coverage for the typed terminal vocabulary: the
    /// current Agent Loop emits Completed/Cancelled/Failed only. These typed
    /// terminal facts are nevertheless part of the native observation contract
    /// and must enter the same absorbing Failed branch exercised end to end by
    /// `failed_activation_preserves_admitted_guidance_for_the_next_activation`.
    #[tokio::test]
    async fn time_and_turn_limit_terminal_observations_map_to_failed_activation() {
        use crate::events::types::AttemptLimit;
        use crate::runtime::identity::AttemptId;

        let attempt_id = AttemptId::new("bounded-attempt");
        let events = [
            RuntimeEvent::AttemptTimedOut {
                attempt_id: attempt_id.clone(),
            },
            RuntimeEvent::AttemptLimitExceeded {
                attempt_id: attempt_id.clone(),
                limit: AttemptLimit::MaxTurns,
            },
            RuntimeEvent::AttemptLimitExceeded {
                attempt_id: attempt_id.clone(),
                limit: AttemptLimit::MaxToolCalls,
            },
            RuntimeEvent::AttemptLimitExceeded {
                attempt_id: attempt_id.clone(),
                limit: AttemptLimit::MaxRuntimeSeconds,
            },
        ];
        for event in events {
            let dir = tempfile::tempdir().unwrap();
            let model = Arc::new(FakeModel::new(Vec::new()));
            let runtime =
                child_test_runtime(&dir, None, None, ConversationId::generate(), model.clone())
                    .await;
            let (_parent, child) = tokio::net::UnixStream::pair().unwrap();
            let (_observation_parent, observation_child) = tokio::net::UnixStream::pair().unwrap();
            let mut dispatcher = ChildControlDispatcher::start(child, observation_child);
            let handle = dispatcher.handle();
            let observations = Arc::new(PendingObservations::new());
            observations.push(ConversationObservation::Event {
                attempt_id: attempt_id.clone(),
                event,
            });
            let terminal = await_terminal(
                &mut dispatcher,
                &runtime,
                &observations,
                &handle,
                &AgentId::new("agent-parent"),
                &mut false,
            )
            .await
            .unwrap();
            assert!(matches!(terminal, AttemptTerminal::Failed(_)));
            assert_eq!(runtime.seal_probe_calls(), 0);
            assert!(model.requests().is_empty());
            dispatcher.shutdown().await;
        }
    }

    /// The failed attempt has already committed when `SealRequested` arrives.
    /// Guidance that won parent admission is then flushed on that same FIFO
    /// lane before `SealGranted`. Its durable acknowledgement cannot reopen the
    /// failed activation; the later activation owns the still-pending input.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[allow(clippy::too_many_lines)] // One failed activation and its durable continuation proof.
    async fn failed_activation_preserves_admitted_guidance_for_the_next_activation() {
        use crate::durable::ConversationStore;
        use crate::model::event::ModelEvent;
        use crate::runtime::subagent::ipc::{GuidanceFrame, read_child_frame, write_parent_frame};

        for (terminal, request_count) in [
            (
                ModelEvent::Failed {
                    error: crate::model::ModelError {
                        kind: crate::model::ModelErrorKind::InvalidRequest,
                        message: "first failure is final".into(),
                        retry_disposition: crate::model::error::ModelRetryDisposition::Never,
                        retry_after_ms: None,
                        provider_code: None,
                        context_overflow: None,
                        malformed_tool_proposal: None,
                        timeout_phase: None,
                        generation: None,
                    },
                },
                1,
            ),
            (
                ModelEvent::Completed {
                    finish_reason: crate::model::finish::ModelFinishReason::Length,
                    usage: None,
                },
                2,
            ),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let id = ConversationId::generate();
            let store =
                Arc::new(crate::durable::SqliteConversationStore::in_memory(id.clone()).unwrap());
            // A length failure owns one bounded corrective generation inside
            // its ordinary attempt. Exhaust it before testing activation reopen.
            let mut scripts = vec![
                vec![
                    FakeStep::Emit(ModelEvent::Started),
                    FakeStep::Emit(terminal)
                ];
                request_count
            ];
            scripts.push(answer("a failed activation must never consume this turn"));
            let model = Arc::new(FakeModel::new(scripts));
            let runtime = child_test_runtime_full(
                &dir,
                None,
                None,
                None,
                None,
                None,
                id.clone(),
                model.clone(),
                Some(store.clone()),
                None,
            )
            .await;
            let (mut parent, child) = tokio::net::UnixStream::pair().unwrap();
            let (_observation_parent, observation_child) = tokio::net::UnixStream::pair().unwrap();
            let observations = Arc::new(PendingObservations::new());
            runtime
                .install_observation_bridge(observations.clone())
                .unwrap();
            runtime.gate_child_turns();
            runtime.activate();
            let serving_runtime = runtime.clone();
            let serve = tokio::spawn(async move {
                let mut dispatcher = ChildControlDispatcher::start(child, observation_child);
                let handle = dispatcher.handle();
                let result = serve_child_delegation(
                    &mut dispatcher,
                    &handle,
                    AgentId::new("agent-parent"),
                    serving_runtime,
                    observations,
                    None,
                )
                .await;
                dispatcher.shutdown().await;
                result
            });
            delegate(&mut parent, "failed delegated task").await;
            assert!(matches!(
                read_child_frame(&mut parent).await.unwrap(),
                Some(ChildFrame::SealRequested)
            ));
            assert_eq!(model.requests().len(), request_count);

            // The parent has closed its admission arbiter. These are the two
            // messages that won admission before the close, delivered FIFO.
            for (guidance_id, message) in [
                (1, "first accepted guidance"),
                (2, "second accepted guidance"),
            ] {
                write_parent_frame(
                    &mut parent,
                    &ParentFrame::Guidance(GuidanceFrame {
                        attachments: Vec::new(),
                        guidance_id,
                        message: message.into(),
                    }),
                )
                .await
                .unwrap();
                assert_eq!(
                    read_child_frame(&mut parent).await.unwrap(),
                    Some(ChildFrame::GuidanceResult(GuidanceResultFrame {
                        guidance_id,
                        outcome: ChildGuidanceOutcome::Accepted,
                    }))
                );
            }
            write_parent_frame(&mut parent, &ParentFrame::SealGranted)
                .await
                .unwrap();
            let failed = read_result(&mut parent).await;
            assert_eq!(failed.status, ChildResultStatus::Failed);
            assert!(failed.content.is_none());
            assert!(failed.diagnostic.unwrap().contains("child attempt failed"));
            assert_eq!(read_child_frame(&mut parent).await.unwrap(), None);
            serve.await.unwrap().unwrap();
            assert_eq!(
                model.requests().len(),
                request_count,
                "failure cannot spend another turn"
            );
            assert_eq!(
                runtime.seal_probe_calls(),
                0,
                "failed attempts never inspect a reopen seal"
            );
            let pending = store.load_pending().unwrap();
            assert_eq!(
                pending.len(),
                2,
                "accepted guidance survives physical drain"
            );
            assert_eq!(adopted_guidance(&runtime), vec!["failed delegated task"]);

            let resumed_model = Arc::new(FakeModel::new(vec![answer("later activation answer")]));
            let resumed = child_test_runtime_full(
                &dir,
                None,
                None,
                None,
                None,
                None,
                id,
                resumed_model.clone(),
                Some(store.clone()),
                None,
            )
            .await;
            let mut fixture = serve_child(&resumed);
            // This synchronous admission attempt has returned through the real
            // pre-Delegate permit check. Absence of a request is not a timeout.
            resumed.admit_now_for_test();
            assert!(!resumed.has_current_attempt());
            assert!(resumed_model.requests().is_empty());
            assert_eq!(store.load_pending().unwrap(), pending);
            delegate(&mut fixture.parent, "later activation task").await;
            let later = read_result(&mut fixture.parent).await;
            assert_eq!(later.status, ChildResultStatus::Succeeded);
            fixture.serve.await.unwrap().unwrap();
            assert_eq!(resumed_model.requests().len(), 1);
            assert_eq!(
                adopted_guidance(&resumed),
                vec![
                    "failed delegated task",
                    "first accepted guidance",
                    "second accepted guidance",
                    "later activation task",
                ]
            );
            assert!(store.load_pending().unwrap().is_empty());
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[allow(clippy::too_many_lines)] // One end-to-end cancellation/observation interleaving.
    async fn cancel_before_completed_observation_never_reopens_activation() {
        use crate::durable::ConversationStore;
        use crate::runtime::subagent::ipc::{GuidanceFrame, read_child_frame, write_parent_frame};
        let dir = tempfile::tempdir().unwrap();
        let id = ConversationId::generate();
        let store =
            Arc::new(crate::durable::SqliteConversationStore::in_memory(id.clone()).unwrap());
        let model = Arc::new(FakeModel::new(vec![answer("first"), answer("forbidden")]));
        let runtime = child_test_runtime_full(
            &dir,
            None,
            None,
            None,
            None,
            None,
            id.clone(),
            model.clone(),
            Some(store.clone()),
            None,
        )
        .await;
        let committed = Arc::new(PendingObservations::new());
        let delivered = Arc::new(PendingObservations::new());
        runtime
            .install_observation_bridge(committed.clone())
            .unwrap();
        runtime.gate_child_turns();
        runtime.activate();
        let (mut parent, child) = tokio::net::UnixStream::pair().unwrap();
        let (_observation_parent, observation_child) = tokio::net::UnixStream::pair().unwrap();
        let cancelled = Arc::new(tokio::sync::Notify::new());
        let serve = tokio::spawn(CANCELLATION_OBSERVED.scope(cancelled.clone(), {
            let runtime = runtime.clone();
            let delivered = delivered.clone();
            async move {
                let mut dispatcher = ChildControlDispatcher::start(child, observation_child);
                let handle = dispatcher.handle();
                let result = serve_child_delegation(
                    &mut dispatcher,
                    &handle,
                    AgentId::new("agent-parent"),
                    runtime,
                    delivered,
                    None,
                )
                .await;
                dispatcher.shutdown().await;
                result
            }
        }));
        delegate(&mut parent, "first task").await;
        let terminal = loop {
            committed.wait().await;
            if let Some(terminal) = committed.drain().into_iter().find(|observation| {
                matches!(
                    observation,
                    ConversationObservation::Event {
                        event: RuntimeEvent::AttemptCompleted { .. },
                        ..
                    }
                )
            }) {
                break terminal;
            }
        };
        write_parent_frame(
            &mut parent,
            &ParentFrame::Guidance(GuidanceFrame {
                attachments: Vec::new(),
                guidance_id: 1,
                message: "pending for a later activation".into(),
            }),
        )
        .await
        .unwrap();
        assert!(matches!(
            read_child_frame(&mut parent).await.unwrap(),
            Some(ChildFrame::GuidanceResult(GuidanceResultFrame {
                outcome: ChildGuidanceOutcome::Accepted,
                ..
            }))
        ));
        let pending = store.load_pending().unwrap();
        assert_eq!(pending.len(), 1);
        write_parent_frame(
            &mut parent,
            &ParentFrame::Cancel {
                reason: Some(CancellationReason::UserRequested),
            },
        )
        .await
        .unwrap();
        cancelled.notified().await;
        delivered.push(terminal);
        let Some(ChildFrame::Result(result)) = read_child_frame(&mut parent).await.unwrap() else {
            panic!("cancelled activation must report its result without reopening negotiation");
        };
        assert_eq!(result.status, ChildResultStatus::Cancelled);
        assert_eq!(read_child_frame(&mut parent).await.unwrap(), None);
        serve.await.unwrap().unwrap();
        assert_eq!(model.requests().len(), 1);
        assert_eq!(runtime.seal_probe_calls(), 0);
        assert_eq!(store.load_pending().unwrap(), pending);
        let next_model = Arc::new(FakeModel::new(vec![answer("explicit continuation")]));
        let resumed = child_test_runtime_full(
            &dir,
            None,
            None,
            None,
            None,
            None,
            id,
            next_model.clone(),
            Some(store.clone()),
            None,
        )
        .await;
        let mut next = serve_child(&resumed);
        delegate(&mut next.parent, "explicit next activation").await;
        assert_eq!(
            read_result(&mut next.parent).await.status,
            ChildResultStatus::Succeeded
        );
        next.serve.await.unwrap().unwrap();
        assert_eq!(next_model.requests().len(), 1);
        assert!(store.load_pending().unwrap().is_empty());
        assert_eq!(
            adopted_guidance(&resumed),
            vec![
                "first task",
                "pending for a later activation",
                "explicit next activation"
            ]
        );
    }

    /// The child has completed one turn and is waiting for `SealGranted`.
    /// Parent FIFO input admits guidance, then Cancel. Cancellation must publish
    /// its terminal before the stale grant, without a second Cancel or reopen.
    /// The fixture retains the transport until that late frame is written;
    /// successful cancellation may otherwise close it before the parent writes.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn cancel_during_close_admission_is_absorbing_and_preserves_guidance() {
        use crate::durable::ConversationStore;
        use crate::runtime::subagent::ipc::{GuidanceFrame, read_child_frame, write_parent_frame};

        let dir = tempfile::tempdir().unwrap();
        let id = ConversationId::generate();
        let store =
            Arc::new(crate::durable::SqliteConversationStore::in_memory(id.clone()).unwrap());
        let model = Arc::new(FakeModel::new(vec![
            answer("first answer"),
            answer("must not run"),
        ]));
        let runtime = child_test_runtime_full(
            &dir,
            None,
            None,
            None,
            None,
            None,
            id,
            model.clone(),
            Some(store.clone()),
            None,
        )
        .await;
        let (mut parent, child) = tokio::net::UnixStream::pair().unwrap();
        let (_observation_parent, observation_child) = tokio::net::UnixStream::pair().unwrap();
        let observations = Arc::new(PendingObservations::new());
        runtime
            .install_observation_bridge(observations.clone())
            .unwrap();
        runtime.gate_child_turns();
        runtime.activate();
        let serving_runtime = runtime.clone();
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
        let serve = tokio::spawn(async move {
            let mut dispatcher = ChildControlDispatcher::start(child, observation_child);
            let handle = dispatcher.handle();
            let result = serve_child_delegation(
                &mut dispatcher,
                &handle,
                AgentId::new("agent-parent"),
                serving_runtime,
                observations,
                None,
            )
            .await;
            // Keep only the fixture transport alive until the parent has
            // sent both FIFO frames and observed the terminal result.
            // Dropping the sender on assertion failure also releases cleanup.
            let _ = shutdown_rx.await;
            dispatcher.shutdown().await;
            result
        });
        delegate(&mut parent, "first task").await;
        assert!(matches!(
            read_child_frame(&mut parent).await.unwrap(),
            Some(ChildFrame::SealRequested)
        ));
        assert_eq!(model.requests().len(), 1);
        write_parent_frame(
            &mut parent,
            &ParentFrame::Guidance(GuidanceFrame {
                attachments: Vec::new(),
                guidance_id: 17,
                message: "accepted for a later activation".into(),
            }),
        )
        .await
        .unwrap();
        assert_eq!(
            read_child_frame(&mut parent).await.unwrap(),
            Some(ChildFrame::GuidanceResult(GuidanceResultFrame {
                guidance_id: 17,
                outcome: ChildGuidanceOutcome::Accepted
            },))
        );
        write_parent_frame(
            &mut parent,
            &ParentFrame::Cancel {
                reason: Some(CancellationReason::UserRequested),
            },
        )
        .await
        .unwrap();
        // Queue the stale grant behind Cancel on the same FIFO lane before
        // observing the result. The transport gate has no semantic role.
        write_parent_frame(&mut parent, &ParentFrame::SealGranted)
            .await
            .unwrap();
        let result = read_result(&mut parent).await;
        assert_eq!(result.status, ChildResultStatus::Cancelled);
        shutdown_tx
            .send(())
            .expect("fixture retains its dispatcher");
        assert_eq!(read_child_frame(&mut parent).await.unwrap(), None);
        serve.await.unwrap().unwrap();
        assert_eq!(model.requests().len(), 1);
        assert_eq!(
            runtime.seal_probe_calls(),
            0,
            "cancel cannot reach the reopen probe"
        );
        assert_eq!(store.load_pending().unwrap().len(), 1);
        assert_eq!(adopted_guidance(&runtime), vec!["first task"]);
    }

    /// Reads the child's one terminal result frame.
    async fn read_result(parent: &mut tokio::net::UnixStream) -> ResultFrame {
        let frame = tokio::time::timeout(
            std::time::Duration::from_secs(20),
            crate::runtime::subagent::ipc::read_child_frame(parent),
        )
        .await
        .expect("child result liveness")
        .expect("child frame")
        .expect("a terminal frame");
        match frame {
            ChildFrame::Result(result) => result,
            other => panic!("expected the one terminal result frame, got {other:?}"),
        }
    }

    /// The parent-authored user messages the child conversation canonically
    /// adopted, in canonical order.
    fn adopted_guidance(runtime: &ConversationRuntime) -> Vec<String> {
        runtime
            .durable_ledger()
            .expect("child ledger")
            .into_iter()
            .filter_map(|block| match block {
                MessageBlock::User(user) if matches!(user.source, UserSource::Agent { .. }) => {
                    Some(
                        user.content
                            .iter()
                            .filter_map(|content| match content {
                                UserContentBlock::Text(text) => Some(text.text.as_str()),
                                _ => None,
                            })
                            .collect::<String>(),
                    )
                }
                _ => None,
            })
            .collect()
    }

    /// **Guidance wins the terminal seal.**
    ///
    /// The seal gate parks the child driver's seal evaluation *before* it
    /// acquires the coordinator lock. At that instant the delegated attempt
    /// has provably completed — it passed its own last inbound safe boundary
    /// and committed `AttemptCompleted`, which is what released the driver —
    /// and the seal has provably not committed. Guidance submitted inside
    /// that window is therefore racing the terminal linearization point
    /// itself, and it must win: the seal reports `Open`, the ordinary
    /// coordinator admits the turn that observes the guidance, and only the
    /// answer of *that* turn is reported to the parent.
    ///
    /// One child, one conversation, one control loop, one terminal result
    /// frame.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn guidance_accepted_before_the_seal_is_observed_before_the_terminal() {
        let dir = tempfile::tempdir().expect("temp root");
        let seal_gate = Arc::new(Gate::default());
        let model = Arc::new(FakeModel::new(vec![
            answer("first answer"),
            answer("answer after the steer"),
        ]));
        let runtime = child_test_runtime_with_seal_gate(
            &dir,
            None,
            None,
            Some(seal_gate.clone()),
            ConversationId::new("conv_cba5be45-4523-71a1-820a-ccdae9d8d33e"),
            model.clone(),
        )
        .await;
        let reopen_gate = Arc::new(Gate::default());
        reopen_gate.arm();
        let mut fixture = serve_child_with_reopen_gate(&runtime, None, Some(reopen_gate.clone()));

        seal_gate.arm();
        delegate(&mut fixture.parent, "delegated task").await;

        // The driver is parked at the seal: the attempt is done, nothing is
        // sealed yet.
        tokio::task::spawn_blocking({
            let seal_gate = Arc::clone(&seal_gate);
            move || seal_gate.wait_entered()
        })
        .await
        .expect("the seal parks after the attempt terminal");
        assert_eq!(model.requests().len(), 1, "exactly one turn has run");

        // The durable acceptance wins the race against the seal.
        runtime
            .submit_parent_guidance(
                UserSource::Agent {
                    agent_id: AgentId::new("agent-parent"),
                },
                vec![UserContentBlock::Text(TextBlock {
                    text: "focus on cancellation ownership".to_owned(),
                })],
            )
            .expect("guidance accepted before the seal commits");
        seal_gate.release();
        tokio::task::spawn_blocking({
            let gate = reopen_gate.clone();
            move || gate.wait_entered()
        })
        .await
        .unwrap();
        assert_eq!(
            model.requests().len(),
            1,
            "no continued model turn before parent Active acknowledgement"
        );
        reopen_gate.release();

        let result = read_result(&mut fixture.parent).await;
        assert_eq!(result.status, ChildResultStatus::Succeeded);
        assert_eq!(
            result.content.as_deref(),
            Some("answer after the steer"),
            "the reported answer is the one that could observe the guidance"
        );
        assert_eq!(
            model.requests().len(),
            2,
            "the accepted guidance received its ordinary model turn"
        );
        assert_eq!(
            adopted_guidance(&runtime),
            vec![
                "delegated task".to_owned(),
                "focus on cancellation ownership".to_owned(),
            ],
            "the guidance entered the SAME child conversation as ordinary \
             parent-authored inbound, after the delegation"
        );
        let last = model.requests().pop().expect("the second request");
        assert!(
            last.messages.iter().any(|message| matches!(
                message,
                crate::model::input::ModelInputMessage::Canonical(MessageBlock::User(user))
                    if user.content.iter().any(|content| matches!(
                        content,
                        UserContentBlock::Text(text)
                            if text.text == "focus on cancellation ownership"
                    ))
            )),
            "the next ordinary model turn observed the guidance"
        );

        // Exactly one terminal frame: the wire closes right after it.
        let after = tokio::time::timeout(
            std::time::Duration::from_secs(20),
            crate::runtime::subagent::ipc::read_child_frame(&mut fixture.parent),
        )
        .await
        .expect("wire close liveness")
        .expect("child frame");
        assert_eq!(after, None, "exactly one terminal result frame is written");
        fixture
            .serve
            .await
            .expect("serve task")
            .expect("serve loop");
    }

    /// **The terminal seal wins.**
    ///
    /// The same gate, released with nothing pending: the seal commits, and
    /// every later guidance submission is refused deterministically with
    /// `GuidanceSealed`. The child reports the answer of the one turn it
    /// ran, and the refused guidance never enters its conversation.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn guidance_after_the_committed_seal_is_deterministically_refused() {
        let dir = tempfile::tempdir().expect("temp root");
        let seal_gate = Arc::new(Gate::default());
        let model = Arc::new(FakeModel::new(vec![answer("only answer")]));
        let runtime = child_test_runtime_with_seal_gate(
            &dir,
            None,
            None,
            Some(seal_gate.clone()),
            ConversationId::new("conv_bbd1ceff-5761-7ba2-8b72-38a9b275c058"),
            model.clone(),
        )
        .await;
        let mut fixture = serve_child(&runtime);

        seal_gate.arm();
        delegate(&mut fixture.parent, "delegated task").await;
        tokio::task::spawn_blocking({
            let seal_gate = Arc::clone(&seal_gate);
            move || seal_gate.wait_entered()
        })
        .await
        .expect("the seal parks after the attempt terminal");
        // Nothing is pending: releasing the gate commits the seal.
        seal_gate.release();

        let result = read_result(&mut fixture.parent).await;
        assert_eq!(result.content.as_deref(), Some("only answer"));

        let refused = runtime
            .submit_parent_guidance(
                UserSource::Agent {
                    agent_id: AgentId::new("agent-parent"),
                },
                vec![UserContentBlock::Text(TextBlock {
                    text: "too late".to_owned(),
                })],
            )
            .expect_err("a sealed conversation refuses guidance");
        assert!(
            matches!(refused, InboundAdmissionError::GuidanceSealed),
            "the refusal names the committed seal, not a timing accident: {refused:?}"
        );
        assert_eq!(
            model.requests().len(),
            1,
            "a refused guidance never opens another model turn"
        );
        assert_eq!(
            adopted_guidance(&runtime),
            vec!["delegated task".to_owned()],
            "a refused guidance never enters the child conversation"
        );
        fixture
            .serve
            .await
            .expect("serve task")
            .expect("serve loop");
    }

    /// **The terminal seal fails closed (Issue #193 review finding #2).**
    ///
    /// A durable read failure is not a proof that the Pending Inbound Inbox
    /// is empty, so it must never be folded into "nothing pending". This
    /// test establishes exactly the window in which that folding would be
    /// observable, and proves the child refuses to seal:
    ///
    /// 1. **the steer is durably accepted** — `submit_parent_guidance`
    ///    returns `Ok` while the seal is parked before the coordinator lock;
    /// 2. **it has not been adopted** — the canonical ledger still contains
    ///    only the delegation, because no attempt has run since;
    /// 3. **the seal evaluates** — the gate is released, and the seal is the
    ///    only caller of the probe it is about to make;
    /// 4. **the durable pending read fails** — one narrow injected fault on
    ///    exactly that probe;
    /// 5. **the seal does not commit** — the child breaks out with a failed
    ///    terminal rather than a sealed one;
    /// 6. **the earlier answer is not published** — the reported frame
    ///    carries no content at all, so `first answer` never reaches the
    ///    parent as a successful terminal;
    /// 7. **the runtime is fail-closed** — the absorbing durability-failure
    ///    fact is committed, and every later inbound admission is refused
    ///    with it;
    /// 8. **exactly one terminal** — the wire closes immediately after the
    ///    single result frame.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn an_unverifiable_pending_inbox_fails_the_terminal_seal_closed() {
        let dir = tempfile::tempdir().expect("temp root");
        let seal_gate = Arc::new(Gate::default());
        let admission_gate = Arc::new(Gate::default());
        let model = Arc::new(FakeModel::new(vec![answer("first answer")]));
        let runtime = child_test_runtime_with_seal_gate(
            &dir,
            None,
            Some(admission_gate.clone()),
            Some(seal_gate.clone()),
            ConversationId::new("conv_3891c656-015e-71df-8476-79495baeeecb"),
            model.clone(),
        )
        .await;
        let mut fixture = serve_child(&runtime);

        seal_gate.arm();
        delegate(&mut fixture.parent, "delegated task").await;
        tokio::task::spawn_blocking({
            let seal_gate = Arc::clone(&seal_gate);
            move || seal_gate.wait_entered()
        })
        .await
        .expect("the seal parks after the attempt terminal");

        // (1) durably accepted, (2) held unadopted: the admission gate parks
        // the coordinator at the entrance of `admit_next_attempt`, before it
        // takes the coordinator lock, so the guidance provably sits in the
        // Pending Inbound Inbox with no attempt admitted for it.
        admission_gate.arm();
        runtime
            .submit_parent_guidance(
                UserSource::Agent {
                    agent_id: AgentId::new("agent-parent"),
                },
                vec![UserContentBlock::Text(TextBlock {
                    text: "unadopted guidance".to_owned(),
                })],
            )
            .expect("guidance accepted before the seal evaluates");
        tokio::task::spawn_blocking({
            let admission_gate = Arc::clone(&admission_gate);
            move || admission_gate.wait_entered()
        })
        .await
        .expect("the coordinator parks before adopting the guidance");
        assert_eq!(
            adopted_guidance(&runtime),
            vec!["delegated task".to_owned()],
            "the accepted guidance is still pending, never adopted"
        );

        // (3)+(4): the only probe the seal makes fails.
        runtime.arm_seal_probe_failures(1);
        seal_gate.release();

        // (5)+(6): no sealed terminal, and no earlier answer published.
        let result = read_result(&mut fixture.parent).await;
        assert_eq!(
            result.status,
            ChildResultStatus::Failed,
            "an unverifiable pending inbox can never produce a successful terminal"
        );
        assert_eq!(
            result.content, None,
            "the answer that predates the accepted guidance is never published"
        );
        let diagnostic = result.diagnostic.clone().expect("a failure diagnostic");
        assert!(
            diagnostic.contains("could not verify the pending inbound inbox"),
            "the diagnostic names the unproven seal, not a semantic failure: {diagnostic}"
        );

        // (7): the runtime followed its existing absorbing fail-closed
        // durability contract.
        let failure = runtime
            .durability_failure()
            .expect("the absorbing durability-failure fact is committed");
        assert_eq!(
            failure.operation,
            crate::runtime::types::DurableOperation::ParentGuidanceSeal,
            "the failure is attributed to the seal's own durable operation"
        );
        assert_eq!(failure.diagnostic, diagnostic);

        // The parked admission may now proceed; the absorbing durability
        // fact refuses it, and the child's drain can complete.
        admission_gate.release();

        // (8): exactly one terminal frame.
        let after = tokio::time::timeout(
            std::time::Duration::from_secs(20),
            crate::runtime::subagent::ipc::read_child_frame(&mut fixture.parent),
        )
        .await
        .expect("wire close liveness")
        .expect("child frame");
        assert_eq!(after, None, "exactly one terminal result frame is written");
        assert_eq!(
            model.requests().len(),
            1,
            "the failed seal never opened another model turn"
        );
        fixture
            .serve
            .await
            .expect("serve task")
            .expect("serve loop");
        assert_eq!(
            adopted_guidance(&runtime),
            vec!["delegated task".to_owned()],
            "the unverifiable guidance is never adopted after the failure"
        );
    }

    /// Parent admission is authoritative. Child-local cancellation cannot
    /// reject a message that already won the registry arbiter. An exact model
    /// arbitration gate holds the attempt while both messages are persisted.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn parent_admitted_guidance_remains_durable_after_child_cancellation_intent() {
        use crate::agent::execution::test_sync::ModelArbitrationPause;

        let dir = tempfile::tempdir().expect("temp root");
        let model = Arc::new(FakeModel::new(vec![vec![
            FakeStep::Emit(crate::model::event::ModelEvent::Started),
            FakeStep::ParkUntilCancelled,
        ]]));
        let (model_pause, mut model_pause_reached, model_pause_release) =
            ModelArbitrationPause::install(1);
        let runtime = child_test_runtime_with_model_pause(
            &dir,
            model_pause,
            ConversationId::new("conv_dfe9945f-baea-7bcf-878b-65b7efa3c297"),
            model.clone(),
        )
        .await;
        let observations = Arc::new(PendingObservations::new());
        runtime
            .install_observation_bridge(Arc::clone(&observations))
            .expect("observation bridge");
        runtime.activate();
        runtime
            .submit_sourced_inbound(
                UserSource::Agent {
                    agent_id: AgentId::new("agent-parent"),
                },
                vec![UserContentBlock::Text(TextBlock {
                    text: "delegated task".to_owned(),
                })],
            )
            .expect("Delegate enters ordinary child inbound");
        // The attempt owns a started provider turn and is held at the exact
        // arbitration barrier, so it cannot settle and clear the
        // current-attempt slot under either admission decision below.
        model_pause_reached
            .wait_for(|is_reached| *is_reached)
            .await
            .expect("model arbitration pause channel stays open");

        // Guidance is legal while the attempt is live and uncancelled.
        runtime
            .submit_parent_guidance(
                UserSource::Agent {
                    agent_id: AgentId::new("agent-parent"),
                },
                vec![UserContentBlock::Text(TextBlock {
                    text: "before cancellation".to_owned(),
                })],
            )
            .expect("a running, uncancelled child accepts guidance");

        // The cancellation intent commits under the one coordinator lock.
        runtime.cancel_current_or_next_attempt(CancellationReason::UserRequested);

        runtime
            .submit_parent_guidance(
                UserSource::Agent {
                    agent_id: AgentId::new("agent-parent"),
                },
                vec![UserContentBlock::Text(TextBlock {
                    text: "after cancellation".to_owned(),
                })],
            )
            .expect("parent-admitted guidance cannot be rejected by child-local cancellation");
        // Only now may the held attempt observe its cancellation and settle.
        let _ = model_pause_release.send(());
        runtime.shutdown().await.expect("child runtime drains");
    }

    // -----------------------------------------------------------------
    // Workflow-owned children are isolated from the generic-steering
    // terminal machinery (Issue #193 architecture review blocker 1)
    // -----------------------------------------------------------------

    /// A real semantic activation over the same on-disk child conversation.
    /// Registry tests forward its emitted Result through the parent publisher.
    #[allow(clippy::too_many_lines)] // One persistent child/IPC/commit interleaving.
    pub(crate) async fn persistent_answer_activation(
        dir: &tempfile::TempDir,
        id: ConversationId,
        mode: &str,
    ) -> ResultFrame {
        use crate::durable::ConversationStore;
        use crate::message::types::ContentBlockIndex;
        use crate::model::event::ModelEvent;
        use crate::model::finish::ModelFinishReason;
        let store = Arc::new(
            crate::durable::SqliteConversationStore::open(
                id.clone(),
                &dir.path().join("answer-child.sqlite"),
            )
            .unwrap(),
        );
        let refusal = vec![
            FakeStep::Emit(ModelEvent::Started),
            FakeStep::Emit(ModelEvent::RefusalDelta {
                block_index: ContentBlockIndex::new(0),
                text: "Current refusal".into(),
            }),
            FakeStep::Emit(ModelEvent::Completed {
                finish_reason: ModelFinishReason::Refusal,
                usage: None,
            }),
        ];
        let scripts = match mode {
            "refusal" => vec![refusal],
            "narration" => {
                let mut narration = workflow_output_answer(serde_json::json!({}));
                for step in &mut narration {
                    match step {
                        FakeStep::Emit(ModelEvent::ToolCallStarted { block_index, call }) => {
                            *block_index = ContentBlockIndex::new(1);
                            call.name = "inspect".into();
                            call.tool_id = crate::runtime::identity::ToolId::new("tool-inspect");
                        }
                        FakeStep::Emit(ModelEvent::ToolCallCompleted { block_index, call }) => {
                            *block_index = ContentBlockIndex::new(1);
                            call.name = "inspect".into();
                            call.tool_id = crate::runtime::identity::ToolId::new("tool-inspect");
                        }
                        _ => {}
                    }
                }
                // Ordinary committed tool narration precedes a valid refusal
                // in a later generation of this same attempt.
                narration.insert(
                    1,
                    FakeStep::Emit(ModelEvent::TextDelta {
                        block_index: ContentBlockIndex::new(0),
                        text: "Earlier tool narration".into(),
                    }),
                );
                vec![narration, refusal]
            }
            "guidance" => vec![
                answer("Earlier guidance attempt"),
                answer("Concluding answer"),
            ],
            text => vec![answer(text)],
        };
        let model = Arc::new(FakeModel::new(scripts));
        let seal = (mode == "guidance").then(|| Arc::new(Gate::default()));
        let mut tools = ToolRegistry::new();
        crate::scripted_suites::support::fake::FakeTool::new(
            crate::scripted_suites::common::tool("inspect", "tool-inspect"),
            crate::scripted_suites::support::fake::success_result("inspected"),
        )
        .register(&mut tools);
        let runtime = child_test_runtime_full(
            dir,
            None,
            None,
            seal.clone(),
            None,
            None,
            id,
            model.clone(),
            Some(store.clone()),
            Some(tools),
        )
        .await;
        let restore = Arc::new(Gate::default());
        let _restore_release = restore.arm_scoped();
        if let Some(seal) = &seal {
            seal.arm();
        } else {
            runtime.install_before_restore_gate(restore.clone());
        }
        let mut fixture = serve_child(&runtime);
        delegate(&mut fixture.parent, "current task").await;
        if let Some(seal) = seal {
            tokio::task::spawn_blocking({
                let gate = seal.clone();
                move || gate.wait_entered()
            })
            .await
            .unwrap();
            runtime
                .submit_parent_guidance(
                    UserSource::Agent {
                        agent_id: AgentId::new("agent-parent"),
                    },
                    vec![UserContentBlock::Text(TextBlock {
                        text: "accepted guidance".into(),
                    })],
                )
                .unwrap();
            seal.release();
        } else {
            tokio::task::spawn_blocking({
                let gate = restore.clone();
                move || gate.wait_entered()
            })
            .await
            .unwrap();
            assert!(
                runtime.has_current_attempt(),
                "coordinator has not restored state"
            );
            let terminal = store
                .read_events(None, 1024)
                .unwrap()
                .events
                .into_iter()
                .rev()
                .find_map(|event| match event.event {
                    RuntimeEvent::AttemptCompleted { attempt_id, .. } => Some(attempt_id),
                    _ => None,
                })
                .unwrap();
            let answer = final_answer(&runtime, &terminal);
            restore.release();
            assert_eq!(
                answer.as_deref(),
                if matches!(mode, "refusal" | "narration") {
                    None
                } else {
                    Some(mode)
                }
            );
        }
        let result = read_result(&mut fixture.parent).await;
        assert_eq!(
            crate::runtime::subagent::ipc::read_child_frame(&mut fixture.parent)
                .await
                .unwrap(),
            None
        );
        fixture.serve.await.unwrap().unwrap();
        assert_eq!(
            model.requests().len(),
            if matches!(mode, "narration" | "guidance") {
                2
            } else {
                1
            }
        );
        assert!(store.load_pending().unwrap().is_empty());
        result
    }

    /// Scripts the one model turn of a Workflow `AgentRun` child: a single
    /// reserved `workflow_output(value)` tool-shaped call (never an
    /// ordinary Tool Plane call) whose arguments satisfy the frozen output
    /// schema, which is what commits the terminal value and completes the
    /// attempt.
    fn workflow_output_answer(value: serde_json::Value) -> Vec<FakeStep> {
        vec![
            FakeStep::Emit(crate::model::event::ModelEvent::Started),
            FakeStep::Emit(crate::model::event::ModelEvent::ToolCallStarted {
                block_index: crate::message::types::ContentBlockIndex::new(0),
                call: crate::tools::types::ToolCallStart {
                    id: crate::runtime::identity::ToolCallId::new("call-workflow-output"),
                    tool_id: crate::runtime::identity::ToolId::new("runtime-workflow-output"),
                    name: crate::runtime::workflow::WORKFLOW_OUTPUT_TOOL_NAME.to_owned(),
                },
            }),
            FakeStep::Emit(crate::model::event::ModelEvent::ToolCallCompleted {
                block_index: crate::message::types::ContentBlockIndex::new(0),
                call: crate::tools::types::ToolCall {
                    id: crate::runtime::identity::ToolCallId::new("call-workflow-output"),
                    tool_id: crate::runtime::identity::ToolId::new("runtime-workflow-output"),
                    name: crate::runtime::workflow::WORKFLOW_OUTPUT_TOOL_NAME.to_owned(),
                    arguments: value,
                },
            }),
            FakeStep::Emit(crate::model::event::ModelEvent::Completed {
                finish_reason: crate::model::finish::ModelFinishReason::ToolCalls,
                usage: None,
            }),
        ]
    }

    /// Runs both semantic orderings through the real reserved output tool,
    /// durable attempt journal and child dispatcher. The registry test forwards
    /// this actual result through its native driver and publication path.
    #[allow(clippy::too_many_lines)]
    pub(crate) async fn workflow_cancellation_ordering(late: bool) -> ResultFrame {
        use crate::durable::ConversationStore;
        use crate::runtime::subagent::ipc::{read_child_frame, write_parent_frame};
        let dir = tempfile::tempdir().unwrap();
        let id = ConversationId::generate();
        let store =
            Arc::new(crate::durable::SqliteConversationStore::in_memory(id.clone()).unwrap());
        let value = serde_json::json!({"summary": "committed answer"});
        let model = Arc::new(FakeModel::new(vec![
            workflow_output_answer(value.clone()),
            answer("forbidden"),
        ]));
        let latch = Arc::new(
            crate::runtime::workflow::WorkflowOutputLatch::new(serde_json::json!({
                "type": "object", "properties": {"summary": {"type": "string"}},
                "required": ["summary"], "additionalProperties": false
            }))
            .unwrap(),
        );
        let (pause, mut before_start, _) = StartBoundaryPause::install(true, false);
        let runtime = child_test_runtime_full(
            &dir,
            if late { None } else { Some(pause) },
            None,
            None,
            Some(latch.clone()),
            None,
            id,
            model.clone(),
            Some(store.clone()),
            None,
        )
        .await;
        let committed = Arc::new(PendingObservations::new());
        let delivered = Arc::new(PendingObservations::new());
        runtime
            .install_observation_bridge(committed.clone())
            .unwrap();
        runtime.gate_child_turns();
        runtime.activate();
        runtime.arm_seal_probe_failures(1);
        let (mut parent, child) = tokio::net::UnixStream::pair().unwrap();
        let (_observation_parent, observation_child) = tokio::net::UnixStream::pair().unwrap();
        let cancelled = Arc::new(tokio::sync::Notify::new());
        let serve = tokio::spawn(CANCELLATION_OBSERVED.scope(cancelled.clone(), {
            let runtime = runtime.clone();
            let delivered = delivered.clone();
            let latch = latch.clone();
            async move {
                let mut dispatcher = ChildControlDispatcher::start(child, observation_child);
                let handle = dispatcher.handle();
                let result = serve_child_delegation(
                    &mut dispatcher,
                    &handle,
                    AgentId::new("agent-parent"),
                    runtime,
                    delivered,
                    Some(latch),
                )
                .await;
                dispatcher.shutdown().await;
                result
            }
        }));
        delegate(&mut parent, "produce workflow output").await;
        let held = if late {
            loop {
                committed.wait().await;
                if let Some(terminal) = committed.drain().into_iter().find(|observation| {
                    matches!(
                        observation,
                        ConversationObservation::Event {
                            event: RuntimeEvent::AttemptCompleted { .. },
                            ..
                        }
                    )
                }) {
                    break Some(terminal);
                }
            }
        } else {
            before_start.as_mut().unwrap().await_park(1).await;
            None
        };
        assert_eq!(latch.committed_value(), late.then(|| value.clone()));
        assert_eq!(
            store
                .read_events(None, 256)
                .unwrap()
                .events
                .iter()
                .filter(|event| matches!(event.event, RuntimeEvent::AttemptCompleted { .. }))
                .count(),
            usize::from(late)
        );
        write_parent_frame(
            &mut parent,
            &ParentFrame::Cancel {
                reason: Some(CancellationReason::UserRequested),
            },
        )
        .await
        .unwrap();
        cancelled.notified().await;
        if let Some(terminal) = held {
            delivered.push(terminal);
        } else {
            before_start.take().unwrap().release();
            loop {
                committed.wait().await;
                let observations = committed.drain();
                let terminal = observations.iter().any(|observation| {
                    matches!(
                        observation,
                        ConversationObservation::Event {
                            event: RuntimeEvent::AttemptCancelled { .. },
                            ..
                        }
                    )
                });
                for observation in observations {
                    delivered.push(observation);
                }
                if terminal {
                    break;
                }
            }
        }
        // Any seal/reopen frame fails this exact protocol assertion.
        let Some(ChildFrame::Result(result)) = read_child_frame(&mut parent).await.unwrap() else {
            panic!("Workflow must finish at its first terminal without seal negotiation");
        };
        assert_eq!(
            result.status,
            if late {
                ChildResultStatus::Succeeded
            } else {
                ChildResultStatus::Cancelled
            }
        );
        assert_eq!(
            result.content,
            late.then(|| serde_json::to_string(&value).unwrap())
        );
        assert_eq!(read_child_frame(&mut parent).await.unwrap(), None);
        serve.await.unwrap().unwrap();
        assert_eq!(latch.committed_value(), late.then_some(value));
        assert_eq!(model.requests().len(), usize::from(late));
        assert_eq!(runtime.seal_probe_calls(), 0);
        result
    }

    /// **Workflow-owned children never enter the parent-guidance terminal
    /// seal** (Issue #193 architecture review blocker 1).
    ///
    /// A Workflow `AgentRun` is structurally not steerable — the generic
    /// control plane refuses it from the ownership fact alone, before any
    /// `Guidance` frame exists — so the steering-specific seal must have no
    /// semantic effect on its lifecycle or terminal result. This test arms
    /// the seal's durable pending-inbox probe to fail **if it is invoked**
    /// and then proves, in order:
    ///
    /// 1. the Workflow child starts and commits a valid `workflow_output`;
    /// 2. the parent-guidance seal probe is never invoked — the counter is
    ///    the direct non-invocation proof, not an inference from the absence
    ///    of an observed failure (the very probe that would fail closed if
    ///    the seal were consulted);
    /// 3. the Workflow attempt completes naturally and still settles through
    ///    its ordinary successful Workflow output contract — `Succeeded`
    ///    carrying the committed value — never a `Failed` terminal derived
    ///    from a steering-only durability probe;
    /// 4. exactly one terminal frame is written;
    /// 5. no Guidance frame or steer content ever entered the child
    ///    conversation: the canonical ledger holds only the delegated task,
    ///    and exactly one model turn ran.
    ///
    /// The isolation is decided from the child's frozen terminal mode (the
    /// `workflow_output` latch), i.e. explicit ownership, never incidental
    /// timing.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_workflow_owned_child_never_consults_the_parent_guidance_seal() {
        let dir = tempfile::tempdir().expect("temp root");
        let committed = serde_json::json!({ "summary": "the workflow node answer" });
        let model = Arc::new(FakeModel::new(vec![workflow_output_answer(
            committed.clone(),
        )]));
        let latch = Arc::new(
            crate::runtime::workflow::WorkflowOutputLatch::new(serde_json::json!({
                "type": "object",
                "properties": {"summary": {"type": "string"}},
                "required": ["summary"],
                "additionalProperties": false
            }))
            .expect("the frozen Workflow output schema compiles"),
        );
        let runtime = child_test_runtime_full(
            &dir,
            None,
            None,
            None,
            Some(Arc::clone(&latch)),
            None,
            ConversationId::new("conv_65454923-c390-7329-8410-7a51296c305b"),
            model.clone(),
            None,
            None,
        )
        .await;
        let mut fixture = serve_child_with_output(&runtime, Some(Arc::clone(&latch)));

        // If the steering-specific seal machinery were consulted after the
        // natural Workflow completion, this armed durable-probe fault would
        // make it fail closed and convert the valid Workflow success into a
        // `Failed` terminal. The fix must keep the probe at zero invocations.
        runtime.arm_seal_probe_failures(1);

        delegate(&mut fixture.parent, "produce the node output").await;

        // (1)+(3)+(4): the Workflow child settles through its normal
        // successful Workflow output contract, exactly once.
        let result = read_result(&mut fixture.parent).await;
        assert_eq!(
            result.status,
            ChildResultStatus::Succeeded,
            "a valid Workflow output completion settles Succeeded"
        );
        assert_eq!(
            result.content.as_deref(),
            Some(
                serde_json::to_string(&committed)
                    .expect("serializable")
                    .as_str()
            ),
            "the reported answer is the committed workflow_output value"
        );
        assert_eq!(
            result.diagnostic, None,
            "no steering-only durability diagnostic can touch a Workflow child"
        );
        assert_eq!(
            latch.committed_value(),
            Some(committed),
            "the latch holds the committed Workflow value"
        );

        // (2): the steering-specific seal probe was never semantically
        // consulted — proven directly by the invocation counter.
        assert_eq!(
            runtime.seal_probe_calls(),
            0,
            "a Workflow-owned child never enters the parent-guidance seal \
             protocol, so its durable probe is never consulted"
        );
        // The armed fault is therefore still armed (never consumed), which
        // is the counterfactual the isolation protects against.
        assert_eq!(
            runtime.durability_failure(),
            None,
            "no durability-failure fact was committed by any steering-only probe"
        );

        // (4): exactly one terminal frame — the wire closes right after it.
        let after = tokio::time::timeout(
            std::time::Duration::from_secs(20),
            crate::runtime::subagent::ipc::read_child_frame(&mut fixture.parent),
        )
        .await
        .expect("wire close liveness")
        .expect("child frame");
        assert_eq!(after, None, "exactly one terminal result frame is written");
        fixture
            .serve
            .await
            .expect("serve task")
            .expect("serve loop");

        // (5): no Guidance frame or steer content is involved — the child
        // conversation adopted exactly the delegated task, and one model
        // turn produced the output.
        assert_eq!(
            model.requests().len(),
            1,
            "the Workflow AgentRun ran exactly its own model turn"
        );
        assert_eq!(
            adopted_guidance(&runtime),
            vec!["produce the node output".to_owned()],
            "the Workflow child's conversation contains only the delegated \
             task — no Guidance frame or generic steer content ever entered it"
        );
    }
}
