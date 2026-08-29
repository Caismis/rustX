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
//! fd 0 (inherited control channel)
//!   -> Hello(spec)      version handshake; mismatch exits before compose
//!   -> dispatcher       the ONE owner of the raw transport from here on
//!   -> compose          the real runtime stack, deny-by-construction, and
//!                       cancellable owned work (Issue #145)
//!   -> Ready            composition and activation complete
//!   -> Delegate(task)   the task enters through the child's ORDINARY
//!                       durable inbound path (UserSource::Agent(parent))
//!   -> observe          the attempt's canonical terminal event
//!   -> Result(candidate) exactly once, bounded
//!   -> drain + exit
//! ```
//!
//! # One control dispatcher (Issue #145)
//!
//! Only the `Hello` version handshake reads the raw `UnixStream` directly:
//! it must be decided before anything at all is composed. Everything after
//! it goes through [`ChildControlDispatcher`], the single owner of the
//! transport, because the child now also creates supervised process units
//! that must offer their containment anchors to the parent concurrently
//! with `Delegate`/`Cancel`/`Result` traffic. No Tool executor and no
//! supervised-unit owner ever touches the stream.
//!
//! # Composition is cancellable owned work (Issue #145)
//!
//! External capability materialization can start an MCP process, verify a
//! content-addressed Python `ToolVersion`, and build a uv environment.
//! Composition therefore races the attempt-derived cancellation and the
//! control channel's EOF: a settled preparation drops the composition, the
//! child never answers `Ready`, no semantic work begins, and the parent
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

use crate::events::types::RuntimeEvent;
use crate::message::content::TextBlock;
use crate::message::types::{MessageBlock, UserContentBlock, UserSource};
use crate::runtime::cancellation::CancellationSignal;
use crate::runtime::observation::{ConversationObservation, PendingObservations};
use crate::runtime::subagent::ipc::{
    ChildFrame, ChildResultStatus, DiagnosticFrame, ParentFrame, ReadyFrame, ResultFrame,
    SUBAGENT_IPC_VERSION, SubagentChildSpec, read_parent_frame, write_child_frame,
};
use crate::runtime::subagent::{MAX_RESULT_CONTENT_BYTES, bound_utf8};

use super::composition::{ChildPreparation, LocalConversationCore, LocalRuntimeDependencies};
use super::dispatcher::{ChildControlDispatcher, ChildControlEvent, ChildControlHandle};

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
    // From here on there is exactly one owner of the raw transport. Every
    // other child-side owner — the semantic driver and every nested
    // supervised process unit — reaches the parent only through it.
    let mut dispatcher = ChildControlDispatcher::start(control);
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

    let code = match Box::pin(run_child(&mut dispatcher, &handle, spec)).await {
        Ok(()) => 0,
        // A startup failure was already reported through `StartupError`
        // by `run_child` when the channel allowed.
        Err(ChildExit::Startup(message)) => {
            let _ = handle
                .send(ChildFrame::StartupError(DiagnosticFrame {
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
) -> Result<(), ChildExit> {
    if let Err(error) = spec.workspace_snapshot.validate() {
        return Err(ChildExit::Startup(format!(
            "the child workspace snapshot is invalid: {error}"
        )));
    }
    if spec.resolved.workspace_policy.is_isolated() != spec.workspace_snapshot.isolated {
        return Err(ChildExit::Startup(
            "the child workspace path and immutable workspace snapshot disagree".to_owned(),
        ));
    }
    let Some(core) = Box::pin(compose_cancellably(dispatcher, handle, &spec)).await? else {
        // Preparation settled (cancellation or parent loss) before the child
        // was owned: nothing composed, nothing started, no result. The
        // parent settles the cancelled/interrupted terminal itself from the
        // physical outcome.
        return Ok(());
    };
    // The observation bridge is installed over the still-inactive runtime,
    // so the attempt's canonical terminal event can never be missed.
    let observations = Arc::new(PendingObservations::new());
    core.runtime()
        .install_observation_bridge(Arc::clone(&observations))
        .map_err(|error| ChildExit::Startup(format!("{error:?}")))?;
    let runtime = core.runtime().clone();
    let headless = core.into_headless();
    drop(headless);
    handle
        .send(ChildFrame::Ready(ReadyFrame {
            subagent_id: spec.subagent_id.clone(),
        }))
        .await
        .map_err(|error| ChildExit::Protocol(error.to_string()))?;
    mark_ready_sent_if_armed();
    serve_child_delegation(
        dispatcher,
        handle,
        spec.parent_agent_id,
        runtime,
        observations,
    )
    .await
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
pub(crate) async fn serve_child_delegation(
    dispatcher: &mut ChildControlDispatcher,
    handle: &ChildControlHandle,
    parent_agent_id: crate::runtime::identity::AgentId,
    runtime: crate::runtime::conversation_runtime::ConversationRuntime,
    observations: Arc<PendingObservations>,
) -> Result<(), ChildExit> {
    // The start gate: no semantic work before the delegation arrives.
    let delegate = match dispatcher.next_event().await {
        Some(ChildControlEvent::Delegate(delegate)) => delegate,
        Some(ChildControlEvent::Cancel { .. }) | None => {
            // Cancelled (or orphaned) before any work began: drain and
            // exit. The parent settles the cancelled/interrupted terminal
            // itself from the physical outcome.
            let _ = runtime.shutdown().await;
            return Ok(());
        }
        Some(ChildControlEvent::ProtocolViolation(message)) => {
            return Err(ChildExit::Protocol(message));
        }
    };

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

    // Observe the attempt to its canonical terminal event while serving
    // Cancel frames through the ordinary cancellation path.
    let terminal = await_terminal(dispatcher, &runtime, &observations).await?;
    let frame = match terminal {
        AttemptTerminal::Completed => {
            let answer = final_answer(&runtime);
            match answer {
                Some(answer) => ResultFrame {
                    status: ChildResultStatus::Succeeded,
                    content: Some(answer),
                    diagnostic: None,
                },
                None => ResultFrame {
                    status: ChildResultStatus::Failed,
                    content: None,
                    diagnostic: Some("the attempt completed without a final answer".to_owned()),
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
            // The parent is gone: drain and exit without a result; the
            // parent's recovery owns the interrupted classification.
            let _ = runtime.shutdown().await;
            return Ok(());
        }
    };
    report_and_drain(handle, &runtime, frame).await
}

/// Composes the child runtime as **cancellable owned work** (Issue #145).
///
/// External capability materialization can take materially longer than the
/// old base-only startup — an MCP process start plus protocol negotiation,
/// a content-addressed `ToolVersion` verification, a uv environment build.
/// Three things therefore race here, and the composition future is dropped
/// the instant any of them wins:
///
/// ```text
/// Cancel from the parent   the spawn attempt no longer wants this child
/// parent control EOF       the parent process is gone
/// composition completes    the child may answer Ready
/// ```
///
/// `Ok(None)` means the preparation settled: nothing was composed, no
/// semantic work began, and the parent settles the terminal from the child's
/// physical outcome.
async fn compose_cancellably(
    dispatcher: &mut ChildControlDispatcher,
    handle: &ChildControlHandle,
    spec: &SubagentChildSpec,
) -> Result<Option<LocalConversationCore>, ChildExit> {
    let cancellation = CancellationSignal::new();
    let preparation = ChildPreparation::new(cancellation.clone(), handle.clone());
    let dependencies = LocalRuntimeDependencies::default();
    let composition =
        LocalConversationCore::compose_subagent_child(spec, &dependencies, &preparation);
    let mut composition = std::pin::pin!(composition);
    let mut events_open = true;
    let composed = loop {
        tokio::select! {
            event = dispatcher.next_event(), if events_open => match event {
                Some(ChildControlEvent::Cancel { .. }) => cancellation.cancel(),
                Some(ChildControlEvent::Delegate(_)) => {
                    return Err(ChildExit::Protocol(
                        "a delegation arrived before the child answered Ready".to_owned(),
                    ));
                }
                Some(ChildControlEvent::ProtocolViolation(message)) => {
                    return Err(ChildExit::Protocol(message));
                }
                None => {
                    // The control channel is finished. The preparation
                    // guard observes the same fact and settles; the arm is
                    // disabled so the loop cannot spin.
                    events_open = false;
                    cancellation.cancel();
                }
            },
            composed = &mut composition => break composed,
        }
    };
    match composed {
        Ok(core) => {
            // The final stretch of composition after the last guarded step
            // is not itself cancellation-checked, so a settlement authority
            // can win the race against the composition's completion. Once
            // pre-commit cancellation has won, `Ready` is impossible:
            // settle the composed runtime — its physical capability
            // runtimes included — and report the settled preparation.
            if cancellation.is_cancelled() || handle.parent_lost() {
                let runtime = core.runtime().clone();
                drop(core);
                let _ = runtime.shutdown().await;
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
        .send(ChildFrame::Result(frame))
        .await
        .map_err(|error| ChildExit::Protocol(error.to_string()))?;
    let _ = runtime.shutdown().await;
    Ok(())
}

/// The canonical terminal of the child's one attempt.
enum AttemptTerminal {
    /// `AttemptCompleted`.
    Completed,
    /// `AttemptCancelled`.
    Cancelled,
    /// `AttemptFailed`, `AttemptTimedOut`, or `AttemptLimitExceeded`, with
    /// the bounded diagnostic.
    Failed(String),
    /// The parent died mid-attempt (control channel EOF).
    Orphaned,
}

/// Drives the attempt to its terminal event, serving cancellation through
/// the ordinary runtime path.
async fn await_terminal(
    dispatcher: &mut ChildControlDispatcher,
    runtime: &crate::runtime::conversation_runtime::ConversationRuntime,
    observations: &Arc<PendingObservations>,
) -> Result<AttemptTerminal, ChildExit> {
    await_terminal_inner(dispatcher, runtime, observations, |_| {}).await
}

#[cfg(test)]
async fn await_terminal_with_probe(
    dispatcher: &mut ChildControlDispatcher,
    runtime: &crate::runtime::conversation_runtime::ConversationRuntime,
    observations: &Arc<PendingObservations>,
    cancellation_before_admission: Arc<tokio::sync::Notify>,
    cancellation_after_admission: Arc<tokio::sync::Notify>,
) -> Result<AttemptTerminal, ChildExit> {
    await_terminal_inner(dispatcher, runtime, observations, move |delivered| {
        if delivered {
            cancellation_after_admission.notify_one();
        } else {
            cancellation_before_admission.notify_one();
        }
    })
    .await
}

async fn await_terminal_inner<F>(
    dispatcher: &mut ChildControlDispatcher,
    runtime: &crate::runtime::conversation_runtime::ConversationRuntime,
    observations: &Arc<PendingObservations>,
    on_cancellation: F,
) -> Result<AttemptTerminal, ChildExit>
where
    F: Fn(bool) + Send + Sync + 'static,
{
    loop {
        tokio::select! {
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
                        let delivered = runtime.cancel_current_or_next_attempt(reason).is_some();
                        on_cancellation(delivered);
                        // The frame is a request, not a terminal fact: the
                        // canonical AttemptCancelled settles the attempt.
                    }
                    Some(ChildControlEvent::Delegate(_)) => {
                        return Err(ChildExit::Protocol(
                            "a second delegation arrived during the attempt".to_owned(),
                        ));
                    }
                    Some(ChildControlEvent::ProtocolViolation(message)) => {
                        return Err(ChildExit::Protocol(message));
                    }
                    None => return Ok(AttemptTerminal::Orphaned),
                }
            }
            () = observations.wait() => {
                for observation in observations.drain() {
                    match observation {
                        ConversationObservation::Event { event, .. } => {
                            match event {
                                RuntimeEvent::AttemptCompleted { .. } => {
                                    return Ok(AttemptTerminal::Completed);
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
) -> Option<String> {
    // The terminal observation fires on the durable commit inside the
    // attempt, before the coordinator's in-memory conversation state is
    // restored — so the answer must be read from the durable authority,
    // where the committed assistant message already exists by definition.
    let ledger = runtime.durable_ledger()?;
    let answer = ledger.iter().rev().find_map(|message| match message {
        MessageBlock::Assistant(assistant) => {
            let text: String = assistant
                .content
                .iter()
                .filter_map(|block| match block {
                    crate::message::types::AssistantContentBlock::Text(text) => {
                        Some(text.text.as_str())
                    }
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("");
            if text.is_empty() { None } else { Some(text) }
        }
        _ => None,
    })?;
    Some(bound_utf8(answer, MAX_RESULT_CONTENT_BYTES))
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

#[cfg(test)]
mod tests {
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
        let workspace = dir.path().join("workspace");
        std::fs::create_dir_all(&workspace).expect("workspace");
        let tool_runtime = ConversationToolRuntime::new(
            conversation_id.clone(),
            &workspace,
            dir.path().join("artifacts"),
        )
        .expect("tool runtime");
        let capability = CapabilityCoordinator::new(CapabilityCoordinatorConfig {
            conversation_id: conversation_id.clone(),
            workspace: tool_runtime.workspace().clone(),
            base_tool_registry: Arc::new(ToolRegistry::new()),
            tool_activation: crate::capabilities::ToolActivationPolicy::default(),
            skill_discovery: crate::skills::SkillDiscoveryConfig::default(),
            mcp_servers: std::collections::BTreeMap::new(),
            base_environment: tool_runtime.environment().clone(),
            environment_store_root: dir.path().join("environments"),
            python_store_roots: None,
        })
        .expect("capability coordinator");
        let candidate = capability.prepare_candidate().await.expect("candidate");
        capability.commit(candidate).expect("capability commit");
        let adapter: Arc<dyn ModelAdapter> = model;
        ConversationRuntime::with_probe(
            RuntimeConversationConfig {
                agent_id: AgentId::new("agent-child"),
                model: scripted_session_model(adapter),
                approval_mode: crate::runtime::ApprovalMode::Policy,
                model_timeout_policy: crate::model::ModelTimeoutPolicy::default(),
                context: ConversationContextConfig {
                    policy: SessionContextPolicy {
                        reserve_tokens: 0,
                        keep_recent_tokens: 0,
                        summary_output_cap: None,
                    },
                    estimator: Arc::new(DefaultTokenEstimator),
                    status_engine: AgentStatusEngine::default(),
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
            },
            CoordinatorProbe {
                start_boundary_pause: start_pause,
                admission_gate,
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
        let conversation_id = ConversationId::new("conv-child-cancel-before-admission");
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
            let mut child_end = ChildControlDispatcher::start(child_end);
            await_terminal_with_probe(
                &mut child_end,
                &child_runtime,
                &child_observations,
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
        let conversation_id = ConversationId::new("conv-child-cancel-pre-start");
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
            let mut child_end = ChildControlDispatcher::start(child_end);
            await_terminal_with_probe(
                &mut child_end,
                &child_runtime,
                &child_observations,
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
        let conversation_id = ConversationId::new("conv-child-cancel-in-flight");
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
            let mut child_end = ChildControlDispatcher::start(child_end);
            await_terminal_with_probe(
                &mut child_end,
                &child_runtime,
                &child_observations,
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

    /// The Issue #145 local race, deterministically in-process (the e2e
    /// module covers the cross-process ordering; see
    /// `local_runtime::issue145_preparation_e2e`): a `Cancel` event sets
    /// the one preparation cancellation signal, and only THEN the guarded
    /// external-preparation step completes. The gate is deliberately
    /// biased to let the completed step win its internal race, so the
    /// composition's own settlement checks are what must hold: the
    /// composition must never be publishable as `Ready`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_step_completing_after_cancellation_never_publishes_ready() {
        let dir = tempfile::tempdir().expect("temp root");
        let workspace = dir.path().join("workspace");
        std::fs::create_dir_all(&workspace).expect("workspace");
        let runtime_root = dir.path().join("child");
        let spec = SubagentChildSpec {
            protocol_version: SUBAGENT_IPC_VERSION,
            subagent_id: crate::runtime::identity::SubagentId::new("conv-issue145-race-subagent-1"),
            child_conversation_id: ConversationId::new("conv-issue145-race-subagent-1"),
            child_agent_id: AgentId::new("agent-child"),
            parent_agent_id: AgentId::new("agent-parent"),
            resolved: crate::runtime::subagent::ResolvedSubagentSpec {
                agent: crate::runtime::subagent::SubagentName::parse("explore")
                    .expect("canonical name"),
                definition_digest: serde_json::from_value(serde_json::json!(
                    "sha256:0000000000000000000000000000000000000000000000000000000000000000"
                ))
                .expect("digest"),
                workspace_policy:
                    crate::runtime::subagent::SubagentWorkspacePolicy::SharedWorkspace,
                instructions: "frozen child instructions".to_owned(),
                model: crate::model::frozen::test_frozen_model_spec(
                    serde_json::from_value(serde_json::json!("local/model")).expect("model ref"),
                ),
                tools: Vec::new(),
                skills: Vec::new(),
                project_instructions: Vec::new(),
                materialization:
                    crate::runtime::subagent::resolver::ResolvedSubagentMaterialization::default(),
            },
            model_timeout_policy: crate::model::ModelTimeoutPolicy::default(),
            agent_status: crate::context::AgentStatusConfig::default(),
            context: SessionContextPolicy {
                reserve_tokens: 0,
                keep_recent_tokens: 0,
                summary_output_cap: None,
            },
            workspace_snapshot: crate::runtime::subagent::WorkspaceSnapshot::shared(
                workspace.clone(),
            ),
            runtime_root: runtime_root.clone(),
        };
        let gate = crate::local_runtime::composition::arm_test_preparation_gate(&runtime_root);

        let (parent, child) = tokio::net::UnixStream::pair().expect("control pair");
        let mut dispatcher = ChildControlDispatcher::start(child);
        let handle = dispatcher.handle();
        let composed = tokio::spawn(async move {
            let outcome = Box::pin(compose_cancellably(&mut dispatcher, &handle, &spec)).await;
            (outcome, dispatcher)
        });

        // 1. The child is provably inside external preparation.
        gate.entered().await;

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
        let (outcome, dispatcher) =
            tokio::time::timeout(std::time::Duration::from_secs(30), composed)
                .await
                .expect("liveness: the settled composition must complete")
                .expect("the composition task must not panic");
        let outcome = outcome.expect("the settled composition is not a startup failure");
        assert!(
            outcome.is_none(),
            "once pre-commit cancellation has won, Ready is impossible"
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
}
