//! Issue #56 deterministic typed-lifecycle regressions.
//!
//! Every test drives a real `AgentExecution` over scripted fixture models and
//! tools and asserts through the canonical Message Ledger, the Conversation
//! Surface, the recorded `RuntimeEvent` trace, the frozen `RequestSnapshot`s,
//! and the recorded provider requests.
//!
//! The suite covers the two seams Issue #56 adds:
//!
//! - `PreStepPolicy` — the typed Enter/Reject boundary between Context
//!   Assembly and the Agent Loop's admission linearization point;
//! - `ToolResultObserver` — the immutable post-structural-settlement
//!   observation of finalized tool results and the deferred post-tool context
//!   it may propose.
//!
//! Every race is established by explicit synchronization (`watch` and the
//! fixture tools' own gates). No test uses `sleep`, timeout, or scheduler luck
//! to establish a claimed interleaving; the bounded waits only contain a
//! broken fixture so the test process fails fast.

use super::{common, support};

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use futures_util::future::BoxFuture;
use tokio::sync::{oneshot, watch};

use crate::runtime::interaction::{
    ApprovalDecision, ApprovalFacts, InteractionCoordinator, InteractionError, InteractionObserver,
    InteractionOutcome, InteractionRequest, InteractionResponse, TestInteractionRendezvous,
};
use rustx::agent::{
    AgentCancellation, AgentExecution, AgentExecutionRequest, AgentExecutionResult,
    AttemptLifecycle, LifecycleError, ObservedToolInvocation, PreStepBatch, PreStepDecision,
    PreStepPolicy, PreToolDecision, PreToolPolicy, PreToolView, ToolResultObservation,
    ToolResultObserver,
};
use rustx::context::{
    ContextAssembly, ContextProposal, ContextRuntime, ContributorInputSnapshot,
    DefaultTokenEstimator, MAX_DEFERRED_CONTEXT_PROPOSALS, MAX_PROPOSALS_PER_CONTRIBUTOR,
    SessionContextPolicy, UserMessageProposal,
};
use rustx::conversation::ConversationState;
use rustx::events::types::{AttemptFailure, AttemptOutcome, RuntimeEvent};
use rustx::message::content::TextBlock;
use rustx::message::types::{
    ContextKind, InboundKind, MessageBlock, UserContentBlock, UserMessageBlock, UserSource,
};
use rustx::model::event::ModelEvent;
use rustx::model::finish::ModelFinishReason;
use rustx::runtime::identity::{
    AgentId, AttemptId, CertifiedExtensionIdentity, ContextContributorIdentity, ConversationId,
    MessageId, NativeContextContributor, ToolCallId, ToolId,
};
use rustx::runtime::types::{CancellationReason, ConversationLifecycle, RuntimeError};
use rustx::tools::executor::{ToolExecutionContext, ToolExecutor, ToolRegistry};
use rustx::tools::types::{
    ToolConcurrencyPolicy, ToolDefinition, ToolExecutionPolicy, ToolExecutionResult,
    ToolExecutionStatus, ToolInvocation, ToolInvocationMode, ToolOrigin, ToolReplayPolicy,
};
use support::fake::{FakeModel, FakeStep, ScriptedCall, fake_model, tool_call_events};

// ---------------------------------------------------------------------------
// Lifecycle fixtures
// ---------------------------------------------------------------------------

/// One recorded proposal batch: the trusted provenance, semantic family, and
/// text of every accepted User context fact the policy observed.
type ObservedBatch = Vec<(UserSource, ContextKind, String)>;

/// A pre-step policy that records the complete final batch it observed and
/// then returns a scripted decision per invocation.
struct ScriptedPolicy {
    decisions: Mutex<std::collections::VecDeque<Result<PreStepDecision, LifecycleError>>>,
    observed: Arc<Mutex<Vec<ObservedBatch>>>,
    /// Optional gate entered before the decision is produced. The sender
    /// reports that the policy is executing; the receiver releases it.
    gate: Option<(watch::Sender<bool>, watch::Receiver<bool>)>,
}

impl ScriptedPolicy {
    fn new(decisions: Vec<Result<PreStepDecision, LifecycleError>>) -> Self {
        Self {
            decisions: Mutex::new(decisions.into()),
            observed: Arc::new(Mutex::new(Vec::new())),
            gate: None,
        }
    }

    fn gated(
        decisions: Vec<Result<PreStepDecision, LifecycleError>>,
        entered: watch::Sender<bool>,
        release: watch::Receiver<bool>,
    ) -> Self {
        Self {
            decisions: Mutex::new(decisions.into()),
            observed: Arc::new(Mutex::new(Vec::new())),
            gate: Some((entered, release)),
        }
    }

    /// The batches this policy observed, one entry per evaluation.
    fn observed(&self) -> Arc<Mutex<Vec<ObservedBatch>>> {
        Arc::clone(&self.observed)
    }
}

impl PreStepPolicy for ScriptedPolicy {
    fn evaluate<'a>(
        &'a self,
        batch: &'a PreStepBatch<'a>,
    ) -> BoxFuture<'a, Result<PreStepDecision, LifecycleError>> {
        let snapshot = batch
            .context
            .user_messages
            .iter()
            .map(|message| {
                (
                    message.source.clone(),
                    message.kind,
                    text_of(&message.content),
                )
            })
            .collect::<Vec<_>>();
        self.observed
            .lock()
            .expect("observed batches lock")
            .push(snapshot);
        let decision = self
            .decisions
            .lock()
            .expect("policy decision lock")
            .pop_front()
            .unwrap_or(Ok(PreStepDecision::Enter));
        Box::pin(async move {
            if let Some((entered, release)) = &self.gate {
                entered.send_replace(true);
                release
                    .clone()
                    .wait_for(|released| *released)
                    .await
                    .expect("policy release channel stays open");
            }
            decision
        })
    }
}

/// The smallest concrete native policy fixture for Issue #64. It records the
/// immutable preflight facts it received, then returns one scripted decision;
/// it has no registry, executor, or canonical mutation authority.
struct ScriptedPreToolPolicy {
    decisions: Mutex<std::collections::VecDeque<Result<PreToolDecision, LifecycleError>>>,
    observed: Arc<Mutex<Vec<ApprovalFacts>>>,
    gate: Option<(watch::Sender<bool>, watch::Receiver<bool>)>,
}

impl ScriptedPreToolPolicy {
    fn new(decisions: Vec<PreToolDecision>) -> Self {
        Self {
            decisions: Mutex::new(decisions.into_iter().map(Ok).collect()),
            observed: Arc::new(Mutex::new(Vec::new())),
            gate: None,
        }
    }

    fn gated(
        decisions: Vec<Result<PreToolDecision, LifecycleError>>,
        entered: watch::Sender<bool>,
        release: watch::Receiver<bool>,
    ) -> Self {
        Self {
            decisions: Mutex::new(decisions.into()),
            observed: Arc::new(Mutex::new(Vec::new())),
            gate: Some((entered, release)),
        }
    }

    fn observed(&self) -> Arc<Mutex<Vec<ApprovalFacts>>> {
        Arc::clone(&self.observed)
    }
}

impl PreToolPolicy for ScriptedPreToolPolicy {
    fn evaluate<'a>(
        &'a self,
        view: &'a PreToolView<'a>,
    ) -> BoxFuture<'a, Result<PreToolDecision, LifecycleError>> {
        self.observed
            .lock()
            .expect("pre-tool facts lock")
            .push(view.approval_facts("recorded pre-tool facts"));
        let decision = self
            .decisions
            .lock()
            .expect("pre-tool decision lock")
            .pop_front()
            .unwrap_or(Ok(PreToolDecision::Allow));
        let gate = self
            .gate
            .as_ref()
            .map(|(entered, release)| (entered.clone(), release.clone()));
        Box::pin(async move {
            if let Some((entered, mut release)) = gate {
                entered.send_replace(true);
                release
                    .wait_for(|released| *released)
                    .await
                    .expect("pre-tool policy release channel stays open");
            }
            decision
        })
    }
}

/// A deterministic interaction owner fixture. Each request publishes its
/// owned facts to the test and waits on the next one-shot response. The Agent
/// Loop still owns the cancellation reference and execution frontier.
struct ScriptedInteractionRendezvous {
    requests: Arc<Mutex<Vec<ApprovalFacts>>>,
    request_count: watch::Sender<usize>,
    responses: Mutex<std::collections::VecDeque<oneshot::Receiver<InteractionOutcome>>>,
    response_gate: Mutex<Option<(watch::Sender<bool>, watch::Receiver<bool>)>>,
}

impl ScriptedInteractionRendezvous {
    fn new(responses: Vec<oneshot::Receiver<InteractionOutcome>>) -> Arc<Self> {
        let (request_count, _receiver) = watch::channel(0);
        Arc::new(Self {
            requests: Arc::new(Mutex::new(Vec::new())),
            request_count,
            responses: Mutex::new(responses.into()),
            response_gate: Mutex::new(None),
        })
    }

    fn gate_after_response(&self, entered: watch::Sender<bool>, release: watch::Receiver<bool>) {
        *self.response_gate.lock().expect("response gate lock") = Some((entered, release));
    }

    fn requests(&self) -> Arc<Mutex<Vec<ApprovalFacts>>> {
        Arc::clone(&self.requests)
    }

    fn subscribe_count(&self) -> watch::Receiver<usize> {
        self.request_count.subscribe()
    }
}

impl TestInteractionRendezvous for ScriptedInteractionRendezvous {
    fn request_approval<'a>(
        &'a self,
        facts: ApprovalFacts,
        cancellation: &'a AgentCancellation,
    ) -> BoxFuture<'a, InteractionOutcome> {
        let count = {
            let mut requests = self.requests.lock().expect("interaction requests lock");
            requests.push(facts);
            requests.len()
        };
        self.request_count.send_replace(count);
        let receiver = self
            .responses
            .lock()
            .expect("interaction response lock")
            .pop_front()
            .expect("one scripted response channel per Ask decision");
        let response_gate = self
            .response_gate
            .lock()
            .expect("response gate lock")
            .take();
        Box::pin(async move {
            let outcome = tokio::select! {
                biased;
                response = receiver => response.unwrap_or(InteractionOutcome::Unavailable),
                () = cancellation.cancelled() => InteractionOutcome::Cancelled {
                    reason: cancellation.reason(),
                },
            };
            if let Some((entered, mut release)) = response_gate {
                entered.send_replace(true);
                release
                    .wait_for(|released| *released)
                    .await
                    .expect("response release channel stays open");
            }
            outcome
        })
    }
}

/// A native coordinator observer used by the cancellation regression. It
/// signals publication through a watch channel, so the test can cancel only
/// after the real interaction owner has admitted the request.
struct NativePendingSignal {
    published: watch::Sender<usize>,
    requests: Arc<Mutex<Vec<InteractionRequest>>>,
}

impl NativePendingSignal {
    fn new() -> (Arc<Self>, watch::Receiver<usize>) {
        let (published, receiver) = watch::channel(0);
        (
            Arc::new(Self {
                published,
                requests: Arc::new(Mutex::new(Vec::new())),
            }),
            receiver,
        )
    }
}

impl InteractionObserver for NativePendingSignal {
    fn on_pending(&self, request: &InteractionRequest) {
        let count = {
            let mut requests = self.requests.lock().expect("native pending requests lock");
            requests.push(request.clone());
            requests.len()
        };
        self.published.send_replace(count);
    }

    fn on_settled(
        &self,
        _interaction_id: &rustx::runtime::InteractionId,
        _outcome: &InteractionOutcome,
    ) {
    }
}

/// One immutable observation recorded by [`RecordingObserver`].
#[derive(Debug, Clone, PartialEq)]
struct RecordedObservation {
    batch_position: usize,
    call_id: ToolCallId,
    tool_id: ToolId,
    origin: ToolOrigin,
    invocation: Option<ObservedToolInvocation>,
    result: ToolExecutionResult,
}

/// A tool-result observer that records every observation and produces the
/// scripted deferred proposals of the call it observed.
struct RecordingObserver {
    /// Deferred proposal texts keyed by canonical call id, in FIFO order.
    proposals: Vec<(&'static str, Vec<&'static str>)>,
    /// A call id whose observation fails instead of proposing.
    fail_on: Option<&'static str>,
    recorded: Arc<Mutex<Vec<RecordedObservation>>>,
    /// Optional gate entered before the first observation settles.
    gate: Option<(watch::Sender<bool>, watch::Receiver<bool>)>,
}

impl RecordingObserver {
    fn new(proposals: Vec<(&'static str, Vec<&'static str>)>) -> Self {
        Self {
            proposals,
            fail_on: None,
            recorded: Arc::new(Mutex::new(Vec::new())),
            gate: None,
        }
    }

    fn failing(fail_on: &'static str) -> Self {
        Self {
            proposals: Vec::new(),
            fail_on: Some(fail_on),
            recorded: Arc::new(Mutex::new(Vec::new())),
            gate: None,
        }
    }

    fn gated(
        proposals: Vec<(&'static str, Vec<&'static str>)>,
        entered: watch::Sender<bool>,
        release: watch::Receiver<bool>,
    ) -> Self {
        Self {
            proposals,
            fail_on: None,
            recorded: Arc::new(Mutex::new(Vec::new())),
            gate: Some((entered, release)),
        }
    }

    /// Parks on the gate and then fails, so a test can make cancellation
    /// observable strictly before the failure is produced.
    fn gated_failing(
        fail_on: &'static str,
        entered: watch::Sender<bool>,
        release: watch::Receiver<bool>,
    ) -> Self {
        Self {
            proposals: Vec::new(),
            fail_on: Some(fail_on),
            recorded: Arc::new(Mutex::new(Vec::new())),
            gate: Some((entered, release)),
        }
    }

    fn recorded(&self) -> Arc<Mutex<Vec<RecordedObservation>>> {
        Arc::clone(&self.recorded)
    }
}

impl ToolResultObserver for RecordingObserver {
    fn observe_tool_result<'a>(
        &'a self,
        observation: &'a ToolResultObservation<'a>,
    ) -> BoxFuture<'a, Result<Vec<UserMessageProposal>, LifecycleError>> {
        self.recorded
            .lock()
            .expect("recorded observations lock")
            .push(RecordedObservation {
                batch_position: observation.batch_position,
                call_id: observation.call_id.clone(),
                tool_id: observation.tool_id.clone(),
                origin: observation.origin.clone(),
                invocation: observation.invocation.cloned(),
                result: observation.result.clone(),
            });
        let call_id = observation.call_id.as_str().to_owned();
        let fails = self.fail_on == Some(call_id.as_str());
        let proposals = self
            .proposals
            .iter()
            .filter(|(id, _)| *id == call_id)
            .flat_map(|(_, texts)| texts.iter().copied())
            .map(proposal)
            .collect::<Vec<_>>();
        Box::pin(async move {
            if let Some((entered, release)) = &self.gate {
                entered.send_replace(true);
                release
                    .clone()
                    .wait_for(|released| *released)
                    .await
                    .expect("observer release channel stays open");
            }
            if fails {
                return Err(LifecycleError::new(format!(
                    "observation of {call_id} failed"
                )));
            }
            Ok(proposals)
        })
    }
}

/// An observer that returns a fixed number of bounded proposals for every
/// observation, used to drive the deferred-context bounds.
struct BulkObserver {
    per_call: usize,
    observations: Arc<AtomicUsize>,
}

impl BulkObserver {
    fn new(per_call: usize) -> Self {
        Self {
            per_call,
            observations: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn observations(&self) -> Arc<AtomicUsize> {
        Arc::clone(&self.observations)
    }
}

impl ToolResultObserver for BulkObserver {
    fn observe_tool_result<'a>(
        &'a self,
        observation: &'a ToolResultObservation<'a>,
    ) -> BoxFuture<'a, Result<Vec<UserMessageProposal>, LifecycleError>> {
        self.observations.fetch_add(1, Ordering::SeqCst);
        let call_id = observation.call_id.as_str().to_owned();
        let per_call = self.per_call;
        Box::pin(async move {
            Ok((0..per_call)
                .map(|index| proposal(&format!("{call_id}-{index}")))
                .collect())
        })
    }
}

/// A fixture tool that settles immediately. Used where a batch only needs to
/// reach structural settlement, without an interleaving to establish.
struct InstantTool {
    definition: ToolDefinition,
}

impl InstantTool {
    fn register(definition: ToolDefinition, registry: &mut ToolRegistry) {
        registry
            .register(definition.clone(), Arc::new(Self { definition }))
            .expect("instant tool registration");
    }
}

impl ToolExecutor for InstantTool {
    fn execute<'a>(
        &'a self,
        _invocation: ToolInvocation,
        _context: ToolExecutionContext<'a>,
    ) -> BoxFuture<'a, ToolExecutionResult> {
        let name = self.definition.name.clone();
        Box::pin(async move {
            ToolExecutionResult {
                status: ToolExecutionStatus::Success,
                content: vec![rustx::tools::types::ToolResultContent::Text(TextBlock {
                    text: format!("{name} done"),
                })],
                duration_ms: 1,
                exit_code: Some(0),
                artifacts: Vec::new(),
                truncation: None,
                managed_output: None,
            }
        })
    }
}

/// A gated fixture tool that records the physical completion order of a
/// whole batch into one shared list.
struct GatedTool {
    definition: ToolDefinition,
    release: watch::Sender<bool>,
    started: watch::Sender<bool>,
    completion_order: Arc<Mutex<Vec<String>>>,
    completed: watch::Sender<bool>,
    invocations: Arc<Mutex<Vec<ToolInvocation>>>,
}

impl GatedTool {
    fn new(
        definition: ToolDefinition,
        completion_order: Arc<Mutex<Vec<String>>>,
    ) -> (Self, GatedToolHandle) {
        let (release, _release_rx) = watch::channel(false);
        let started = watch::Sender::new(false);
        let completed = watch::Sender::new(false);
        let handle = GatedToolHandle {
            release: release.clone(),
            started: started.subscribe(),
            completed: completed.subscribe(),
            invocations: Arc::new(Mutex::new(Vec::new())),
        };
        let invocations = Arc::clone(&handle.invocations);
        (
            Self {
                definition,
                release,
                started,
                completion_order,
                completed,
                invocations,
            },
            handle,
        )
    }

    fn register(self, registry: &mut ToolRegistry) {
        registry
            .register(self.definition.clone(), Arc::new(self))
            .expect("gated tool registration");
    }
}

/// The test-side control handle of one [`GatedTool`].
struct GatedToolHandle {
    release: watch::Sender<bool>,
    started: watch::Receiver<bool>,
    completed: watch::Receiver<bool>,
    invocations: Arc<Mutex<Vec<ToolInvocation>>>,
}

const GATED_TOOL_LIVENESS_GUARD: std::time::Duration = std::time::Duration::from_secs(120);

impl GatedToolHandle {
    async fn await_started(&mut self) {
        tokio::time::timeout(
            GATED_TOOL_LIVENESS_GUARD,
            self.started.wait_for(|started| *started),
        )
        .await
        .expect("gated tool start wait exceeded liveness guard")
        .expect("gated tool start channel stays open");
    }

    async fn release_and_await_completion(&mut self) {
        self.release.send_replace(true);
        tokio::time::timeout(
            GATED_TOOL_LIVENESS_GUARD,
            self.completed.wait_for(|completed| *completed),
        )
        .await
        .expect("gated tool completion wait exceeded liveness guard")
        .expect("gated tool completion channel stays open");
    }

    fn invocations(&self) -> Vec<ToolInvocation> {
        self.invocations
            .lock()
            .expect("gated invocation lock")
            .clone()
    }
}

impl ToolExecutor for GatedTool {
    fn execute<'a>(
        &'a self,
        invocation: ToolInvocation,
        _context: ToolExecutionContext<'a>,
    ) -> BoxFuture<'a, ToolExecutionResult> {
        let started = self.started.clone();
        let mut release = self.release.subscribe();
        self.invocations
            .lock()
            .expect("gated invocation lock")
            .push(invocation.clone());
        Box::pin(async move {
            started.send_replace(true);
            release
                .wait_for(|released| *released)
                .await
                .expect("gated tool release channel stays open");
            self.completion_order
                .lock()
                .expect("completion order lock")
                .push(invocation.tool_name.clone());
            self.completed.send_replace(true);
            ToolExecutionResult {
                status: ToolExecutionStatus::Success,
                content: vec![rustx::tools::types::ToolResultContent::Text(TextBlock {
                    text: format!("{} done", invocation.tool_name),
                })],
                duration_ms: 1,
                exit_code: Some(0),
                artifacts: Vec::new(),
                truncation: None,
                managed_output: None,
            }
        })
    }
}

// ---------------------------------------------------------------------------
// Shared fixtures
// ---------------------------------------------------------------------------

/// One bounded deferred User context proposal.
fn proposal(text: &str) -> UserMessageProposal {
    UserMessageProposal {
        content: vec![UserContentBlock::Text(TextBlock {
            text: text.to_owned(),
        })],
    }
}

/// A lifecycle whose only deferred-context producer is the native runtime
/// observation owner.
fn native_lifecycle(observer: Arc<dyn ToolResultObserver>) -> AttemptLifecycle {
    AttemptLifecycle::inert()
        .with_native_tool_result_observer(observer)
        .expect("the native owner is unclaimed")
}

/// A lifecycle whose only deferred-context producer is one certified
/// extension.
fn extension_lifecycle(key: &str, observer: Arc<dyn ToolResultObserver>) -> AttemptLifecycle {
    AttemptLifecycle::inert()
        .with_extension_tool_result_observer(
            CertifiedExtensionIdentity::new(key).expect("identity"),
            observer,
        )
        .expect("the extension is unclaimed")
}

fn extension_source(key: &str) -> UserSource {
    UserSource::Extension {
        contributor: CertifiedExtensionIdentity::new(key).expect("identity"),
    }
}

/// A lifecycle binding a native observer and one extension observer.
fn native_and_extension_lifecycle(
    native: Arc<dyn ToolResultObserver>,
    key: &str,
    extension: Arc<dyn ToolResultObserver>,
    native_first: bool,
) -> AttemptLifecycle {
    let identity = CertifiedExtensionIdentity::new(key).expect("identity");
    if native_first {
        AttemptLifecycle::inert()
            .with_native_tool_result_observer(native)
            .expect("native owner")
            .with_extension_tool_result_observer(identity, extension)
            .expect("extension owner")
    } else {
        AttemptLifecycle::inert()
            .with_extension_tool_result_observer(identity, extension)
            .expect("extension owner")
            .with_native_tool_result_observer(native)
            .expect("native owner")
    }
}

/// A Context Assembly that has **certified** the given extension. This is the
/// one semantic admission authority; binding a lifecycle observer to the same
/// logical key proves nothing without it.
fn assembly_certifying(key: &'static str, attestation: Option<&'static str>) -> ContextAssembly {
    let mut assembly = ContextAssembly::new();
    assembly
        .register_extension(
            key,
            attestation.map(str::to_owned),
            Arc::new(|_: &ContributorInputSnapshot| Ok(Vec::new())),
        )
        .expect("register certified extension");
    assembly
}

fn text_of(content: &[UserContentBlock]) -> String {
    content
        .iter()
        .map(|block| match block {
            UserContentBlock::Text(text) => text.text.clone(),
            UserContentBlock::Image(_) | UserContentBlock::File(_) => String::new(),
        })
        .collect()
}

fn parallel_tool(name: &str, id: &str) -> ToolDefinition {
    common::tool_policies(
        name,
        id,
        ToolExecutionPolicy::ForegroundOnly,
        ToolConcurrencyPolicy::Parallel,
    )
}

fn scripted_call(id: &str, tool_id: &'static str, name: &'static str) -> ScriptedCall {
    ScriptedCall {
        id: Box::leak(id.to_owned().into_boxed_str()),
        tool_id,
        name,
        arguments: serde_json::json!({}),
    }
}

/// A model script: one tool-call turn with the given calls, then one plain
/// stop turn.
fn tool_turn_then_stop(calls: &[ScriptedCall]) -> Vec<Vec<FakeStep>> {
    let mut first = vec![FakeStep::Emit(ModelEvent::Started)];
    for (index, call) in calls.iter().enumerate() {
        for event in tool_call_events(u32::try_from(index).expect("small batch"), call) {
            first.push(FakeStep::Emit(event));
        }
    }
    first.push(FakeStep::Emit(ModelEvent::Completed {
        finish_reason: ModelFinishReason::ToolCalls,
        usage: None,
    }));
    vec![
        first,
        vec![
            FakeStep::Emit(ModelEvent::Started),
            FakeStep::Emit(ModelEvent::TextDelta {
                block_index: rustx::message::types::ContentBlockIndex::new(0),
                text: "done".to_owned(),
            }),
            FakeStep::Emit(ModelEvent::Completed {
                finish_reason: ModelFinishReason::Stop,
                usage: None,
            }),
        ],
    ]
}

/// A single stop turn.
fn stop_turn() -> Vec<Vec<FakeStep>> {
    vec![vec![
        FakeStep::Emit(ModelEvent::Started),
        FakeStep::Emit(ModelEvent::TextDelta {
            block_index: rustx::message::types::ContentBlockIndex::new(0),
            text: "done".to_owned(),
        }),
        FakeStep::Emit(ModelEvent::Completed {
            finish_reason: ModelFinishReason::Stop,
            usage: None,
        }),
    ]]
}

fn request(conversation_id: ConversationId, model: &Arc<FakeModel>) -> AgentExecutionRequest {
    AgentExecutionRequest {
        agent_id: AgentId::new("agent-a"),
        conversation_id,
        attempt_id: AttemptId::new("attempt-1"),
        conversation: ConversationState::from_messages(vec![MessageBlock::User(
            UserMessageBlock {
                id: MessageId::new("msg-user-1"),
                content: vec![UserContentBlock::Text(TextBlock {
                    text: "go".to_owned(),
                })],
                source: UserSource::Human,
                kind: InboundKind::Message,
                timestamp: None,
            },
        )])
        .expect("bootstrap conversation"),
        initial_turn_trigger: rustx::agent::InitialTurnTrigger::Continuation,
        timezone: None,
        model: support::attempt_model(model.clone(), "fake-model"),
    }
}

/// The Surface revision of the bootstrap conversation alone, before any
/// dynamic context could advance it.
fn bootstrap_revision() -> rustx::conversation::SurfaceRevision {
    ConversationState::from_messages(vec![MessageBlock::User(UserMessageBlock {
        id: MessageId::new("msg-user-1"),
        content: vec![UserContentBlock::Text(TextBlock {
            text: "go".to_owned(),
        })],
        source: UserSource::Human,
        kind: InboundKind::Message,
        timestamp: None,
    })])
    .expect("bootstrap conversation")
    .revision()
}

fn context_runtime(model: &Arc<FakeModel>, assembly: ContextAssembly) -> ContextRuntime {
    ContextRuntime::for_attempt_with_assembly(
        SessionContextPolicy {
            reserve_tokens: 0,
            keep_recent_tokens: 0,
            summary_output_cap: None,
        },
        Arc::new(DefaultTokenEstimator),
        rustx::context::AgentStatusComposer::default(),
        assembly,
        &support::attempt_model(model.clone(), "fake-model"),
    )
    .expect("valid context runtime")
}

/// A contributor producing exactly one bounded proposal with the given text.
fn contributor(
    text: &'static str,
    invocations: Arc<AtomicUsize>,
) -> Arc<dyn rustx::context::ContextContributor> {
    Arc::new(move |_: &ContributorInputSnapshot| {
        invocations.fetch_add(1, Ordering::SeqCst);
        Ok(vec![ContextProposal::UserMessage(proposal(text))])
    })
}

/// Runs one attempt over the given registry, assembly, and lifecycle.
async fn run(
    model: &Arc<FakeModel>,
    tools: ToolRegistry,
    assembly: ContextAssembly,
    lifecycle: AttemptLifecycle,
    cancellation: &AgentCancellation,
) -> common::DurableExecutionAudit {
    let tool_runtime = common::tool_runtime("conv-issue56");
    let store = tool_runtime.durable_store();
    let capability = common::capability_lease(tools, &tool_runtime).await;
    let result = AgentExecution::new(
        request(tool_runtime.conversation_id().clone(), model),
        capability.into_lease(),
        cancellation,
        context_runtime(model, assembly),
        &tool_runtime,
        lifecycle,
    )
    .expect("conversation identity matches the tool runtime")
    .run()
    .await;
    common::durable_agent_result(result, store.as_ref())
}

/// A compact canonical description of the committed Message Ledger.
fn ledger_shape(result: &AgentExecutionResult) -> Vec<String> {
    result
        .messages()
        .iter()
        .map(|message| match message {
            MessageBlock::User(user) => match &user.kind {
                InboundKind::Context(kind) => {
                    format!("context({kind:?}):{}", text_of(&user.content))
                }
                other => format!("user({other:?}):{}", text_of(&user.content)),
            },
            MessageBlock::Assistant(assistant) => {
                let calls = assistant
                    .content
                    .iter()
                    .filter_map(|block| match block {
                        rustx::message::types::AssistantContentBlock::ToolCall(call) => {
                            Some(call.id.as_str().to_owned())
                        }
                        _ => None,
                    })
                    .collect::<Vec<_>>();
                if calls.is_empty() {
                    "assistant".to_owned()
                } else {
                    format!("assistant({})", calls.join(","))
                }
            }
            MessageBlock::Tool(tool) => format!("tool_result({})", tool.tool_call_id),
            MessageBlock::System(_) => "system".to_owned(),
        })
        .collect()
}

fn terminal_events(events: &[RuntimeEvent]) -> Vec<&RuntimeEvent> {
    events
        .iter()
        .filter(|event| {
            matches!(
                event,
                RuntimeEvent::AttemptCompleted { .. }
                    | RuntimeEvent::AttemptCancelled { .. }
                    | RuntimeEvent::AttemptTimedOut { .. }
                    | RuntimeEvent::AttemptLimitExceeded { .. }
                    | RuntimeEvent::AttemptFailed { .. }
            )
        })
        .collect()
}

/// Asserts exactly one terminal event and that it is the last event.
fn assert_single_terminal(events: &[RuntimeEvent]) -> &RuntimeEvent {
    let terminals = terminal_events(events);
    assert_eq!(terminals.len(), 1, "exactly one terminal event");
    assert_eq!(
        events.last(),
        Some(terminals[0]),
        "the terminal event is last"
    );
    terminals[0]
}

fn tool_messages(result: &AgentExecutionResult) -> Vec<&rustx::message::types::ToolMessageBlock> {
    result
        .messages()
        .iter()
        .filter_map(|message| match message {
            MessageBlock::Tool(tool) => Some(tool),
            _ => None,
        })
        .collect()
}

/// Runs one real `AgentExecution` while its pre-tool policy is parked. The
/// controller cancels only after the policy reports entry, then releases the
/// policy with the scripted result. The returned interaction count proves
/// whether an Ask was consumed after the post-await cancellation checkpoint.
async fn run_cancelled_after_pending_pre_tool_policy(
    decision: Result<PreToolDecision, LifecycleError>,
) -> (common::DurableExecutionAudit, usize, GatedToolHandle) {
    let (policy_entered, mut policy_entered_rx) = watch::channel(false);
    let (policy_release, policy_release_rx) = watch::channel(false);
    let policy = Arc::new(ScriptedPreToolPolicy::gated(
        vec![decision],
        policy_entered,
        policy_release_rx,
    ));
    let rendezvous = ScriptedInteractionRendezvous::new(Vec::new());
    let mut interaction_count = rendezvous.subscribe_count();
    let mut tools = ToolRegistry::new();
    let (tool, tool_handle) = GatedTool::new(
        parallel_tool("alpha", "tool-alpha"),
        Arc::new(Mutex::new(Vec::new())),
    );
    tool.register(&mut tools);
    let model = fake_model(tool_turn_then_stop(&[scripted_call(
        "call-a",
        "tool-alpha",
        "alpha",
    )]));
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let controller_cancellation = cancellation.clone();
    let controller = tokio::spawn(async move {
        policy_entered_rx
            .wait_for(|entered| *entered)
            .await
            .expect("pre-tool policy entered");
        assert!(controller_cancellation.request_cancel(CancellationReason::RuntimeShutdown));
        policy_release.send_replace(true);
    });
    let result = run(
        &model,
        tools,
        ContextAssembly::new(),
        AttemptLifecycle::inert()
            .with_pre_tool_policy(policy)
            .with_test_interaction_rendezvous(rendezvous),
        &cancellation,
    )
    .await;
    controller.await.expect("policy cancellation controller");
    (result, *interaction_count.borrow_and_update(), tool_handle)
}

fn assert_cancelled_tool_slot_without_start(
    result: &common::DurableExecutionAudit,
    _interaction_count: usize,
    tool_handle: &GatedToolHandle,
) {
    assert!(
        tool_handle.invocations().is_empty(),
        "executor was not invoked"
    );
    assert!(!*tool_handle.started.borrow(), "executor did not start");
    let messages = tool_messages(result);
    assert_eq!(messages.len(), 1, "one structural tool result slot");
    assert!(matches!(
        &messages[0].result.status,
        ToolExecutionStatus::Cancelled {
            reason: CancellationReason::RuntimeShutdown
        }
    ));
    assert_eq!(
        result
            .event_history
            .iter()
            .filter(|event| matches!(event, RuntimeEvent::ToolExecutionStarted { .. }))
            .count(),
        0,
        "cancellation before start emits no ToolExecutionStarted"
    );
    assert!(
        !result
            .event_history
            .iter()
            .any(|event| matches!(event, RuntimeEvent::ToolExecutionCompleted { .. }))
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancellation_after_pending_pre_tool_ask_is_cancelled_without_publishing_interaction() {
    let (result, interaction_count, tool_handle) =
        run_cancelled_after_pending_pre_tool_policy(Ok(PreToolDecision::Ask {
            reason: "approval required".to_owned(),
        }))
        .await;

    assert!(matches!(
        result.outcome,
        AttemptOutcome::Cancelled {
            reason: CancellationReason::RuntimeShutdown
        }
    ));
    assert_eq!(interaction_count, 0, "no Ask interaction was published");
    assert_cancelled_tool_slot_without_start(&result, interaction_count, &tool_handle);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancellation_after_pending_pre_tool_deny_does_not_consume_deny() {
    let (result, interaction_count, tool_handle) =
        run_cancelled_after_pending_pre_tool_policy(Ok(PreToolDecision::Deny {
            reason: "policy denial".to_owned(),
        }))
        .await;

    assert!(matches!(
        result.outcome,
        AttemptOutcome::Cancelled {
            reason: CancellationReason::RuntimeShutdown
        }
    ));
    assert_eq!(interaction_count, 0, "no Ask interaction was published");
    assert_cancelled_tool_slot_without_start(&result, interaction_count, &tool_handle);
    assert!(!result.messages().iter().any(|message| {
        matches!(
            message,
            MessageBlock::Tool(tool)
                if matches!(tool.result.status, ToolExecutionStatus::Denied { .. })
        )
    }));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancellation_after_pending_pre_tool_error_does_not_fail_closed_as_denied() {
    let (result, interaction_count, tool_handle) = run_cancelled_after_pending_pre_tool_policy(
        Err(LifecycleError::new("scripted policy error")),
    )
    .await;

    assert!(matches!(
        result.outcome,
        AttemptOutcome::Cancelled {
            reason: CancellationReason::RuntimeShutdown
        }
    ));
    assert_eq!(interaction_count, 0, "no Ask interaction was published");
    assert_cancelled_tool_slot_without_start(&result, interaction_count, &tool_handle);
    assert!(!result.messages().iter().any(|message| {
        matches!(
            message,
            MessageBlock::Tool(tool)
                if matches!(tool.result.status, ToolExecutionStatus::Denied { .. })
        )
    }));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn native_ask_cancellation_settles_through_the_real_coordinator() {
    let lifecycle = ConversationLifecycle::new();
    assert!(lifecycle.activate());
    let coordinator = Arc::new(InteractionCoordinator::new(
        ConversationId::new("conv-issue56"),
        lifecycle,
    ));
    coordinator.set_provider_available(true);
    let (observer, mut published) = NativePendingSignal::new();
    coordinator.install_observer(observer.clone());

    let mut tools = ToolRegistry::new();
    let (tool, tool_handle) = GatedTool::new(
        parallel_tool("alpha", "tool-alpha"),
        Arc::new(Mutex::new(Vec::new())),
    );
    tool.register(&mut tools);
    let model = fake_model(tool_turn_then_stop(&[scripted_call(
        "call-native-ask",
        "tool-alpha",
        "alpha",
    )]));
    let policy = Arc::new(ScriptedPreToolPolicy::new(vec![PreToolDecision::Ask {
        reason: "native approval".to_owned(),
    }]));
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let controller_cancellation = cancellation.clone();
    let observer_for_controller = observer.clone();
    let (id_sender, id_receiver) = oneshot::channel();
    let controller = tokio::spawn(async move {
        published
            .wait_for(|count| *count == 1)
            .await
            .expect("native interaction publication");
        let id = observer_for_controller
            .requests
            .lock()
            .expect("native pending requests lock")
            .first()
            .expect("one native pending request")
            .id
            .clone();
        id_sender.send(id).expect("send native interaction id");
        assert!(controller_cancellation.request_cancel(CancellationReason::RuntimeShutdown));
    });

    let result = run(
        &model,
        tools,
        ContextAssembly::new(),
        AttemptLifecycle::inert()
            .with_pre_tool_policy(policy)
            .with_native_interaction(coordinator.clone()),
        &cancellation,
    )
    .await;
    let interaction_id = id_receiver.await.expect("native interaction id");
    controller.await.expect("native cancellation controller");

    assert!(matches!(
        result.outcome,
        AttemptOutcome::Cancelled {
            reason: CancellationReason::RuntimeShutdown
        }
    ));
    assert_eq!(
        observer
            .requests
            .lock()
            .expect("native pending requests lock")
            .len(),
        1,
        "the real coordinator published Ask"
    );
    assert_eq!(coordinator.pending_count(), 0);
    assert_eq!(
        coordinator.respond(
            &interaction_id,
            InteractionResponse::Approval {
                decision: ApprovalDecision::Allow,
            },
        ),
        Err(InteractionError::NotPending {
            interaction_id: interaction_id.clone()
        })
    );
    assert_cancelled_tool_slot_without_start(&result, 1, &tool_handle);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn answered_allow_is_rechecked_against_cancellation_before_tool_start() {
    let (response_sender, response_receiver) = oneshot::channel();
    let rendezvous = ScriptedInteractionRendezvous::new(vec![response_receiver]);
    let (answered, mut answered_rx) = watch::channel(false);
    let (release, release_rx) = watch::channel(false);
    rendezvous.gate_after_response(answered, release_rx);

    let mut tools = ToolRegistry::new();
    let (tool, tool_handle) = GatedTool::new(
        parallel_tool("alpha", "tool-alpha"),
        Arc::new(Mutex::new(Vec::new())),
    );
    tool.register(&mut tools);
    let model = fake_model(tool_turn_then_stop(&[scripted_call(
        "call-allow-race",
        "tool-alpha",
        "alpha",
    )]));
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let controller_cancellation = cancellation.clone();
    let controller = tokio::spawn(async move {
        answered_rx
            .wait_for(|entered| *entered)
            .await
            .expect("Answered(Allow) terminal outcome");
        assert!(controller_cancellation.request_cancel(CancellationReason::RuntimeShutdown));
        release.send_replace(true);
    });
    let mut interaction_count = rendezvous.subscribe_count();
    let run_task = tokio::spawn(async move {
        run(
            &model,
            tools,
            ContextAssembly::new(),
            AttemptLifecycle::inert()
                .with_pre_tool_policy(Arc::new(ScriptedPreToolPolicy::new(vec![
                    PreToolDecision::Ask {
                        reason: "allow race".to_owned(),
                    },
                ])))
                .with_test_interaction_rendezvous(rendezvous),
            &cancellation,
        )
        .await
    });

    interaction_count
        .wait_for(|count| *count == 1)
        .await
        .expect("Ask interaction publication");
    response_sender
        .send(InteractionOutcome::Answered {
            response: InteractionResponse::Approval {
                decision: ApprovalDecision::Allow,
            },
        })
        .expect("Answer(Allow) response");
    let result = run_task.await.expect("Agent Execution task");
    controller.await.expect("allow cancellation controller");

    assert!(matches!(
        result.outcome,
        AttemptOutcome::Cancelled {
            reason: CancellationReason::RuntimeShutdown
        }
    ));
    assert_eq!(
        *interaction_count.borrow_and_update(),
        1,
        "Answered(Allow) came from one published Ask"
    );
    assert_cancelled_tool_slot_without_start(
        &result,
        *interaction_count.borrow_and_update(),
        &tool_handle,
    );
}

// ---------------------------------------------------------------------------
// PreStepPolicy
// ---------------------------------------------------------------------------

/// A contributor proposal that a later pre-step policy rejects leaves no
/// canonical trace at all: no committed context, no Surface advancement, no
/// frozen `RequestSnapshot`, and no provider request.
#[tokio::test]
async fn pre_step_reject_commits_no_context_and_starts_no_request() {
    let model = fake_model(stop_turn());
    let invocations = Arc::new(AtomicUsize::new(0));
    let mut assembly = ContextAssembly::new();
    assembly
        .register_extension(
            "example.extension",
            Some("package-v1".to_owned()),
            contributor("extension context", Arc::clone(&invocations)),
        )
        .expect("register contributor");
    let policy = Arc::new(ScriptedPolicy::new(vec![Ok(PreStepDecision::Reject {
        reason: "policy said no".to_owned(),
    })]));
    let observed = policy.observed();
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let result = run(
        &model,
        ToolRegistry::new(),
        assembly,
        AttemptLifecycle::inert().with_pre_step_policy(policy),
        &cancellation,
    )
    .await;

    assert_eq!(
        invocations.load(Ordering::SeqCst),
        1,
        "the contributor ran exactly once"
    );
    let batches = observed.lock().expect("observed batches lock").clone();
    assert_eq!(batches.len(), 1, "the policy evaluated exactly one batch");
    assert_eq!(
        batches[0],
        vec![(
            UserSource::Extension {
                contributor: rustx::runtime::identity::CertifiedExtensionIdentity::new(
                    "example.extension"
                )
                .expect("identity"),
            },
            ContextKind::ExtensionEnvironment,
            "extension context".to_owned(),
        )],
        "the policy observed the exact final proposal batch"
    );
    assert!(matches!(
        &result.outcome,
        AttemptOutcome::Failed {
            error: AttemptFailure::Runtime {
                error: RuntimeError::PreStepRejected { reason },
            },
        } if reason == "policy said no"
    ));
    assert_eq!(
        ledger_shape(&result),
        vec!["user(Message):go".to_owned()],
        "no proposed dynamic context became canonical"
    );
    assert_eq!(
        result.conversation.revision(),
        bootstrap_revision(),
        "the Surface never advanced because of the rejected proposals"
    );
    assert!(
        result.snapshot_history().is_empty(),
        "a rejected step freezes no RequestSnapshot"
    );
    assert!(
        model.requests().is_empty(),
        "a rejected step issues no provider request"
    );
    assert_single_terminal(&result.event_history);
}

/// No contributor has a private path around the policy: native and
/// extension proposals alike reach the same downstream evaluation, and a
/// rejection stops all of them together.
#[tokio::test]
async fn every_contributor_proposal_reaches_the_same_policy() {
    let model = fake_model(stop_turn());
    let invocations = Arc::new(AtomicUsize::new(0));
    let mut assembly = ContextAssembly::new();
    // Registration order is deliberately the reverse of logical order.
    assembly
        .register_extension(
            "zeta.extension",
            None,
            contributor("zeta context", Arc::clone(&invocations)),
        )
        .expect("register zeta");
    assembly
        .register_extension(
            "alpha.extension",
            None,
            contributor("alpha context", Arc::clone(&invocations)),
        )
        .expect("register alpha");
    let policy = Arc::new(ScriptedPolicy::new(vec![Ok(PreStepDecision::Reject {
        reason: "no step".to_owned(),
    })]));
    let observed = policy.observed();
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let result = run(
        &model,
        ToolRegistry::new(),
        assembly,
        AttemptLifecycle::inert().with_pre_step_policy(policy),
        &cancellation,
    )
    .await;

    assert_eq!(invocations.load(Ordering::SeqCst), 2);
    let batches = observed.lock().expect("observed batches lock").clone();
    assert_eq!(
        batches[0]
            .iter()
            .map(|(_, _, text)| text.clone())
            .collect::<Vec<_>>(),
        vec!["alpha context".to_owned(), "zeta context".to_owned()],
        "the policy sees every proposal in stable logical contributor order"
    );
    assert_eq!(ledger_shape(&result), vec!["user(Message):go".to_owned()]);
    assert!(model.requests().is_empty());
    assert_single_terminal(&result.event_history);
}

/// A failing policy is contained exactly like a rejection: nothing is
/// admitted, no request starts, and the attempt settles once.
#[tokio::test]
async fn pre_step_policy_failure_admits_nothing_and_settles_once() {
    let model = fake_model(stop_turn());
    let invocations = Arc::new(AtomicUsize::new(0));
    let mut assembly = ContextAssembly::new();
    assembly
        .register_extension(
            "example.extension",
            None,
            contributor("extension context", Arc::clone(&invocations)),
        )
        .expect("register contributor");
    let policy = Arc::new(ScriptedPolicy::new(vec![Err(LifecycleError::new(
        "policy exploded",
    ))]));
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let result = run(
        &model,
        ToolRegistry::new(),
        assembly,
        AttemptLifecycle::inert().with_pre_step_policy(policy),
        &cancellation,
    )
    .await;

    assert!(matches!(
        &result.outcome,
        AttemptOutcome::Failed {
            error: AttemptFailure::Runtime {
                error: RuntimeError::PreStepPolicyFailed { message },
            },
        } if message == "policy exploded"
    ));
    assert_eq!(ledger_shape(&result), vec!["user(Message):go".to_owned()]);
    assert!(result.snapshot_history().is_empty());
    assert!(model.requests().is_empty());
    assert_single_terminal(&result.event_history);
}

/// Cancellation observed while a bounded policy evaluation is pending does
/// not cancel the policy: the evaluation settles, and the loop's own generic
/// pre-admission checkpoint still owns admission. The interleaving is
/// established by the policy's own gate, never by timing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancellation_during_pending_pre_step_policy_wins_admission() {
    let model = fake_model(stop_turn());
    let invocations = Arc::new(AtomicUsize::new(0));
    let mut assembly = ContextAssembly::new();
    assembly
        .register_extension(
            "example.extension",
            None,
            contributor("extension context", Arc::clone(&invocations)),
        )
        .expect("register contributor");
    let (entered, mut entered_rx) = watch::channel(false);
    let (release, release_rx) = watch::channel(false);
    let policy = Arc::new(ScriptedPolicy::gated(
        vec![Ok(PreStepDecision::Enter)],
        entered,
        release_rx,
    ));
    let observed = policy.observed();
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let controller_cancellation = cancellation.clone();
    let controller = tokio::spawn(async move {
        entered_rx
            .wait_for(|entered| *entered)
            .await
            .expect("policy entered");
        // Cancellation becomes observable strictly while the policy is
        // parked, i.e. after Context Assembly finished and before the
        // pre-admission checkpoint runs.
        controller_cancellation.cancel();
        release.send_replace(true);
    });
    let result = run(
        &model,
        ToolRegistry::new(),
        assembly,
        AttemptLifecycle::inert().with_pre_step_policy(policy),
        &cancellation,
    )
    .await;
    controller.await.expect("cancellation controller");

    assert_eq!(
        observed.lock().expect("observed batches lock").len(),
        1,
        "the policy evaluated exactly once and was not restarted"
    );
    assert!(matches!(
        result.outcome,
        AttemptOutcome::Cancelled {
            reason: CancellationReason::UserRequested
        }
    ));
    assert_eq!(ledger_shape(&result), vec!["user(Message):go".to_owned()]);
    assert!(result.snapshot_history().is_empty());
    assert!(model.requests().is_empty());
    assert_single_terminal(&result.event_history);
}

// ---------------------------------------------------------------------------
// Tool batch ordering and deferred post-tool context
// ---------------------------------------------------------------------------

/// Two parallel calls whose physical completion order is deliberately
/// inverted still commit canonical results in original model call order.
///
/// The interleaving is exact: `B` is released and its completion is awaited
/// while `A` is still parked on its own gate, so the recorded physical order
/// is `[beta, alpha]` by construction, never by scheduling luck.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn parallel_tool_results_commit_in_canonical_call_order() {
    let (result, physical) = run_inverted_parallel_batch(
        AttemptLifecycle::inert(),
        CancellationReason::UserRequested,
        None,
    )
    .await;

    assert_eq!(
        physical,
        vec!["beta".to_owned(), "alpha".to_owned()],
        "B physically completed before A"
    );
    assert_eq!(
        ledger_shape(&result),
        vec![
            "user(Message):go".to_owned(),
            "assistant(call-a,call-b)".to_owned(),
            "tool_result(call-a)".to_owned(),
            "tool_result(call-b)".to_owned(),
            "assistant".to_owned(),
        ],
        "canonical order follows model call order, not completion order"
    );
    assert_single_terminal(&result.event_history);
}

/// Deferred post-tool context never interleaves between sibling tool
/// results: the canonical prefix is the complete result batch, and only then
/// the admitted context, in canonical call order.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn deferred_post_tool_context_never_interleaves_between_sibling_results() {
    let observer = Arc::new(RecordingObserver::new(vec![
        ("call-a", vec!["context A1"]),
        ("call-b", vec!["context B1"]),
    ]));
    let recorded = observer.recorded();
    let (result, physical) = run_inverted_parallel_batch(
        native_lifecycle(observer),
        CancellationReason::UserRequested,
        None,
    )
    .await;

    assert_eq!(physical, vec!["beta".to_owned(), "alpha".to_owned()]);
    assert_eq!(
        ledger_shape(&result),
        vec![
            "user(Message):go".to_owned(),
            "assistant(call-a,call-b)".to_owned(),
            "tool_result(call-a)".to_owned(),
            "tool_result(call-b)".to_owned(),
            "context(RuntimeToolObservation):context A1".to_owned(),
            "context(RuntimeToolObservation):context B1".to_owned(),
            "assistant".to_owned(),
        ],
        "the structural prefix is the complete result batch; context follows"
    );
    let observations = recorded.lock().expect("recorded lock").clone();
    assert_eq!(
        observations
            .iter()
            .map(|observation| observation.batch_position)
            .collect::<Vec<_>>(),
        vec![0, 1],
        "observation runs in canonical batch order"
    );
    // The admitted deferred context is explained by the native
    // tool-result-observation owner inside the frozen context generation, so
    // historical reconstruction never needs the observer again.
    let second = &result.snapshot_history()[1];
    assert!(
        second.context_generation.contributors.iter().any(|entry| {
            entry.identity
                == ContextContributorIdentity::Native(
                    NativeContextContributor::RuntimeToolObservation,
                )
        }),
        "the frozen generation records the deferred-context owner"
    );
    assert_single_terminal(&result.event_history);
}

/// Multiple proposals per observation keep exact FIFO order inside a call,
/// and calls keep canonical batch order between each other, even when the
/// later call physically completed first.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn deferred_post_tool_context_preserves_fifo_within_and_across_calls() {
    let observer = Arc::new(RecordingObserver::new(vec![
        ("call-a", vec!["A1", "A2"]),
        ("call-b", vec!["B1", "B2"]),
    ]));
    let (result, physical) = run_inverted_parallel_batch(
        native_lifecycle(observer),
        CancellationReason::UserRequested,
        None,
    )
    .await;

    assert_eq!(physical, vec!["beta".to_owned(), "alpha".to_owned()]);
    assert_eq!(
        ledger_shape(&result)
            .into_iter()
            .filter(|entry| entry.starts_with("context("))
            .collect::<Vec<_>>(),
        vec![
            "context(RuntimeToolObservation):A1".to_owned(),
            "context(RuntimeToolObservation):A2".to_owned(),
            "context(RuntimeToolObservation):B1".to_owned(),
            "context(RuntimeToolObservation):B2".to_owned(),
        ],
        "order is (canonical call position, proposal FIFO)"
    );
}

/// Deferred post-tool context is ordinary transient context: a later
/// pre-step rejection stops it exactly like any contributor proposal, so the
/// observer is not a privileged committer.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn deferred_post_tool_context_cannot_bypass_a_later_policy_rejection() {
    let observer = Arc::new(RecordingObserver::new(vec![(
        "call-a",
        vec!["deferred context"],
    )]));
    let policy = Arc::new(ScriptedPolicy::new(vec![
        Ok(PreStepDecision::Enter),
        Ok(PreStepDecision::Reject {
            reason: "second step rejected".to_owned(),
        }),
    ]));
    let policy_batches = policy.observed();
    let (result, _physical) = run_inverted_parallel_batch(
        native_lifecycle(observer).with_pre_step_policy(policy),
        CancellationReason::UserRequested,
        None,
    )
    .await;

    let batches = policy_batches
        .lock()
        .expect("observed batches lock")
        .clone();
    assert_eq!(batches.len(), 2, "one evaluation per primary step");
    assert_eq!(
        batches[1],
        vec![(
            UserSource::Runtime,
            ContextKind::RuntimeToolObservation,
            "deferred context".to_owned(),
        )],
        "the deferred proposal is evaluated by the same policy authority"
    );
    assert!(matches!(
        &result.outcome,
        AttemptOutcome::Failed {
            error: AttemptFailure::Runtime {
                error: RuntimeError::PreStepRejected { reason },
            },
        } if reason == "second step rejected"
    ));
    assert_eq!(
        ledger_shape(&result),
        vec![
            "user(Message):go".to_owned(),
            "assistant(call-a,call-b)".to_owned(),
            "tool_result(call-a)".to_owned(),
            "tool_result(call-b)".to_owned(),
        ],
        "the rejected deferred context never became canonical"
    );
    assert_eq!(
        result.snapshot_history().len(),
        1,
        "the rejected second step froze no snapshot"
    );
    assert_single_terminal(&result.event_history);
}

// ---------------------------------------------------------------------------
// Native pre-tool interaction and Tool Plane settlement
// ---------------------------------------------------------------------------

/// An approval response resumes the original Tool Plane invocation. The
/// recorded facts, executor invocation, Assistant `ToolCall`, and canonical
/// `ToolMessage` all retain the same identity and validated arguments.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn approval_allow_executes_the_exact_prepared_invocation() {
    let policy = Arc::new(ScriptedPreToolPolicy::new(vec![PreToolDecision::Ask {
        reason: "native approval".to_owned(),
    }]));
    let observed_policy = policy.observed();
    let (response_tx, response_rx) = oneshot::channel();
    let rendezvous = ScriptedInteractionRendezvous::new(vec![response_rx]);
    let requests = rendezvous.requests();
    let mut request_count = rendezvous.subscribe_count();

    let mut tools = ToolRegistry::new();
    let (tool, mut tool_handle) = GatedTool::new(
        parallel_tool("alpha", "tool-alpha"),
        Arc::new(Mutex::new(Vec::new())),
    );
    tool.register(&mut tools);
    let model = fake_model(tool_turn_then_stop(&[scripted_call(
        "call-a",
        "tool-alpha",
        "alpha",
    )]));
    let cancellation = AgentCancellation::new(CancellationReason::RuntimeShutdown);
    let task = {
        let model = Arc::clone(&model);
        let cancellation = cancellation.clone();
        let lifecycle = AttemptLifecycle::inert()
            .with_pre_tool_policy(policy)
            .with_test_interaction_rendezvous(rendezvous);
        tokio::spawn(async move {
            run(
                &model,
                tools,
                ContextAssembly::new(),
                lifecycle,
                &cancellation,
            )
            .await
        })
    };

    request_count
        .wait_for(|count| *count == 1)
        .await
        .expect("approval request published");
    let facts = requests
        .lock()
        .expect("interaction requests lock")
        .first()
        .cloned()
        .expect("approval facts");
    assert_eq!(facts.call_id, ToolCallId::new("call-a"));
    assert_eq!(facts.tool_id, ToolId::new("tool-alpha"));
    assert_eq!(facts.tool_name, "alpha");
    assert_eq!(facts.origin, ToolOrigin::Builtin);
    assert_eq!(facts.mode, ToolInvocationMode::Foreground);
    assert_eq!(facts.arguments, serde_json::json!({}));
    assert_eq!(facts.reason, "native approval");
    assert_eq!(observed_policy.lock().expect("policy facts lock").len(), 1);
    assert!(!*tool_handle.started.borrow(), "Allow has not arrived yet");

    response_tx
        .send(InteractionOutcome::Answered {
            response: InteractionResponse::Approval {
                decision: ApprovalDecision::Allow,
            },
        })
        .expect("approval waiter remains live");
    tool_handle.await_started().await;
    assert_eq!(
        tool_handle.invocations(),
        vec![ToolInvocation {
            call_id: ToolCallId::new("call-a"),
            tool_id: ToolId::new("tool-alpha"),
            tool_name: "alpha".to_owned(),
            mode: ToolInvocationMode::Foreground,
            arguments: serde_json::json!({}),
        }]
    );
    tool_handle.release_and_await_completion().await;
    let result = task.await.expect("attempt task");

    let calls = result
        .messages()
        .iter()
        .filter_map(|message| match message {
            MessageBlock::Assistant(assistant) => Some(assistant),
            _ => None,
        })
        .flat_map(|assistant| assistant.content.iter())
        .filter_map(|block| match block {
            rustx::message::types::AssistantContentBlock::ToolCall(call) => Some(call),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].id, ToolCallId::new("call-a"));
    assert_eq!(calls[0].tool_id, ToolId::new("tool-alpha"));
    assert_eq!(calls[0].arguments, serde_json::json!({}));
    let messages = tool_messages(&result);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].tool_call_id, calls[0].id);
    assert_eq!(messages[0].tool_id, calls[0].tool_id);
    assert!(matches!(
        messages[0].result.status,
        ToolExecutionStatus::Success
    ));
    assert_eq!(
        result
            .event_history
            .iter()
            .filter(|event| matches!(event, RuntimeEvent::ToolExecutionStarted { .. }))
            .count(),
        1
    );
}

/// Denial is a typed Tool Plane result slot, not executor failure: the
/// executor and start event are absent while the normal batch still settles.
#[tokio::test]
async fn approval_deny_emits_one_denied_result_without_starting_executor() {
    let policy = Arc::new(ScriptedPreToolPolicy::new(vec![PreToolDecision::Deny {
        reason: "human denied".to_owned(),
    }]));
    let mut tools = ToolRegistry::new();
    let (tool, tool_handle) = GatedTool::new(
        parallel_tool("alpha", "tool-alpha"),
        Arc::new(Mutex::new(Vec::new())),
    );
    tool.register(&mut tools);
    let model = fake_model(tool_turn_then_stop(&[scripted_call(
        "call-a",
        "tool-alpha",
        "alpha",
    )]));
    let cancellation = AgentCancellation::new(CancellationReason::RuntimeShutdown);
    let result = run(
        &model,
        tools,
        ContextAssembly::new(),
        AttemptLifecycle::inert().with_pre_tool_policy(policy),
        &cancellation,
    )
    .await;

    assert!(matches!(result.outcome, AttemptOutcome::Completed { .. }));
    assert!(
        !*tool_handle.started.borrow(),
        "denial never starts an executor"
    );
    assert!(tool_handle.invocations().is_empty());
    assert_eq!(
        result
            .event_history
            .iter()
            .filter(|event| matches!(event, RuntimeEvent::ToolExecutionStarted { .. }))
            .count(),
        0
    );
    let messages = tool_messages(&result);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].tool_call_id, ToolCallId::new("call-a"));
    assert_eq!(messages[0].tool_id, ToolId::new("tool-alpha"));
    assert!(matches!(
        &messages[0].result.status,
        ToolExecutionStatus::Denied { reason } if reason == "human denied"
    ));
}

/// The strong parallel-batch contract resolves every approval in canonical
/// call order before any executor start frontier advances. A denied sibling
/// receives a result slot but never receives an executor future.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn parallel_batch_resolves_approvals_before_any_tool_start() {
    let policy = Arc::new(ScriptedPreToolPolicy::new(vec![
        PreToolDecision::Ask {
            reason: "first".to_owned(),
        },
        PreToolDecision::Ask {
            reason: "second".to_owned(),
        },
    ]));
    let (first_tx, first_rx) = oneshot::channel();
    let (second_tx, second_rx) = oneshot::channel();
    let rendezvous = ScriptedInteractionRendezvous::new(vec![first_rx, second_rx]);
    let mut request_count = rendezvous.subscribe_count();
    let mut tools = ToolRegistry::new();
    let (alpha, mut alpha_handle) = GatedTool::new(
        parallel_tool("alpha", "tool-alpha"),
        Arc::new(Mutex::new(Vec::new())),
    );
    let (beta, beta_handle) = GatedTool::new(
        parallel_tool("beta", "tool-beta"),
        Arc::new(Mutex::new(Vec::new())),
    );
    alpha.register(&mut tools);
    beta.register(&mut tools);
    let model = fake_model(tool_turn_then_stop(&[
        scripted_call("call-a", "tool-alpha", "alpha"),
        scripted_call("call-b", "tool-beta", "beta"),
    ]));
    let cancellation = AgentCancellation::new(CancellationReason::RuntimeShutdown);
    let task = {
        let model = Arc::clone(&model);
        let cancellation = cancellation.clone();
        let lifecycle = AttemptLifecycle::inert()
            .with_pre_tool_policy(policy)
            .with_test_interaction_rendezvous(rendezvous);
        tokio::spawn(async move {
            run(
                &model,
                tools,
                ContextAssembly::new(),
                lifecycle,
                &cancellation,
            )
            .await
        })
    };

    request_count
        .wait_for(|count| *count == 1)
        .await
        .expect("first approval request");
    first_tx
        .send(InteractionOutcome::Answered {
            response: InteractionResponse::Approval {
                decision: ApprovalDecision::Allow,
            },
        })
        .expect("first approval waiter");
    request_count
        .wait_for(|count| *count == 2)
        .await
        .expect("second approval request");
    assert!(!*alpha_handle.started.borrow());
    assert!(!*beta_handle.started.borrow());
    second_tx
        .send(InteractionOutcome::Answered {
            response: InteractionResponse::Approval {
                decision: ApprovalDecision::Deny {
                    reason: "second denied".to_owned(),
                },
            },
        })
        .expect("second approval waiter");
    alpha_handle.await_started().await;
    assert!(
        !*beta_handle.started.borrow(),
        "denied sibling never starts"
    );
    alpha_handle.release_and_await_completion().await;
    let result = task.await.expect("attempt task");

    let messages = tool_messages(&result);
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].tool_call_id, ToolCallId::new("call-a"));
    assert_eq!(messages[1].tool_call_id, ToolCallId::new("call-b"));
    assert!(matches!(
        messages[0].result.status,
        ToolExecutionStatus::Success
    ));
    assert!(matches!(
        &messages[1].result.status,
        ToolExecutionStatus::Denied { reason } if reason == "second denied"
    ));
}

/// Invalid business arguments are rejected by `ToolRegistry` preflight before
/// the policy boundary. Approval never becomes a schema-validation bypass.
#[tokio::test]
async fn invalid_preflight_never_reaches_pre_tool_policy() {
    let policy = Arc::new(ScriptedPreToolPolicy::new(Vec::new()));
    let observed = policy.observed();
    let mut tools = ToolRegistry::new();
    let definition = ToolDefinition {
        id: ToolId::new("tool-alpha"),
        name: "alpha".to_owned(),
        description: "requires a path".to_owned(),
        input_schema: serde_json::json!({
            "type": "object",
            "required": ["path"],
            "properties": {"path": {"type": "string"}}
        }),
        execution_policy: ToolExecutionPolicy::ForegroundOnly,
        concurrency_policy: ToolConcurrencyPolicy::Sequential,
        replay_policy: ToolReplayPolicy::Never,
        origin: ToolOrigin::Builtin,
    };
    InstantTool::register(definition, &mut tools);
    let model = fake_model(tool_turn_then_stop(&[ScriptedCall {
        id: "call-a",
        tool_id: "tool-alpha",
        name: "alpha",
        arguments: serde_json::json!({}),
    }]));
    let cancellation = AgentCancellation::new(CancellationReason::RuntimeShutdown);
    let result = run(
        &model,
        tools,
        ContextAssembly::new(),
        AttemptLifecycle::inert().with_pre_tool_policy(policy),
        &cancellation,
    )
    .await;

    assert!(observed.lock().expect("policy facts lock").is_empty());
    assert!(matches!(
        tool_messages(&result)[0].result.status,
        ToolExecutionStatus::Failed { .. }
    ));
}

// ---------------------------------------------------------------------------
// Observer immutability, identity, and failure containment
// ---------------------------------------------------------------------------

/// The observer sees exactly the finalized result that is committed, the
/// committed result does not change afterwards, the Assistant `ToolCall`
/// identity/arguments are untouched, and exactly one `ToolMessage` exists per
/// call.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn observer_sees_the_exact_committed_result_and_changes_nothing() {
    let observer = Arc::new(RecordingObserver::new(Vec::new()));
    let recorded = observer.recorded();
    let (result, _physical) = run_inverted_parallel_batch(
        native_lifecycle(observer),
        CancellationReason::UserRequested,
        None,
    )
    .await;

    let observations = recorded.lock().expect("recorded lock").clone();
    let committed = tool_messages(&result);
    assert_eq!(observations.len(), 2);
    assert_eq!(committed.len(), 2, "exactly one ToolMessage per call");
    for (observation, message) in observations.iter().zip(&committed) {
        assert_eq!(&observation.call_id, &message.tool_call_id);
        assert_eq!(&observation.tool_id, &message.tool_id);
        assert_eq!(
            &observation.result, &message.result,
            "the observed result is the committed result"
        );
        assert_eq!(observation.origin, ToolOrigin::Builtin);
        assert_eq!(
            observation
                .invocation
                .as_ref()
                .map(|invocation| invocation.mode),
            Some(ToolInvocationMode::Foreground)
        );
    }
    let calls = result
        .messages()
        .iter()
        .filter_map(|message| match message {
            MessageBlock::Assistant(assistant) => Some(assistant),
            _ => None,
        })
        .flat_map(|assistant| assistant.content.iter())
        .filter_map(|block| match block {
            rustx::message::types::AssistantContentBlock::ToolCall(call) => Some(call),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].id, ToolCallId::new("call-a"));
    assert_eq!(calls[0].arguments, serde_json::json!({}));
    assert_eq!(calls[1].id, ToolCallId::new("call-b"));
    assert_eq!(calls[1].arguments, serde_json::json!({}));
}

/// A consumer distinguishes the native rustX Read capability from an
/// unrelated tool whose public model-facing name is also `read`, using only
/// the stable typed identity the observation carries.
///
/// The observation deliberately does not expose the model-facing tool name,
/// so a name comparison is not even expressible.
#[tokio::test]
async fn native_read_is_distinguished_from_a_non_native_tool_named_read() {
    /// The classification a future certified context consumer (Issue #58)
    /// would perform, expressed only through stable typed identity.
    fn is_native_read(observation: &RecordedObservation) -> bool {
        observation.tool_id == ToolId::new("tool-read") && observation.origin == ToolOrigin::Builtin
    }

    // 1. The real native tool plane: the actual registered Read capability.
    let fixture = common::native_fixture();
    std::fs::write(
        fixture.runtime.workspace().root().join("notes.txt"),
        "hello\n",
    )
    .expect("workspace file");
    let native_observer = Arc::new(RecordingObserver::new(Vec::new()));
    let native_recorded = native_observer.recorded();
    let model = fake_model(tool_turn_then_stop(&[ScriptedCall {
        id: "call-read",
        tool_id: "tool-read",
        name: "read",
        arguments: serde_json::json!({
            "path": fixture
                .runtime
                .workspace()
                .root()
                .join("notes.txt")
                .to_str()
                .expect("utf8"),
        }),
    }]));
    let capability = common::capability_lease(fixture.registry.clone(), &fixture.runtime).await;
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let native_result = AgentExecution::new(
        request(fixture.runtime.conversation_id().clone(), &model),
        capability.into_lease(),
        &cancellation,
        context_runtime(&model, ContextAssembly::new()),
        &fixture.runtime,
        native_lifecycle(native_observer),
    )
    .expect("conversation identity matches the tool runtime")
    .run()
    .await;
    assert!(matches!(
        native_result.outcome,
        AttemptOutcome::Completed { .. }
    ));
    let native = native_recorded.lock().expect("recorded lock").clone();
    assert_eq!(native.len(), 1);
    assert!(
        is_native_read(&native[0]),
        "the registered native Read capability is recognized"
    );

    // 2. A non-native tool whose public name is also `read`.
    let mut tools = ToolRegistry::new();
    let mut impostor = common::tool_policies(
        "read",
        "tool-mcp-read",
        ToolExecutionPolicy::ForegroundOnly,
        ToolConcurrencyPolicy::Sequential,
    );
    impostor.origin = ToolOrigin::Mcp {
        server_id: rustx::runtime::identity::McpServerId::new("mcp-fs"),
    };
    impostor.replay_policy = ToolReplayPolicy::Never;
    let order = Arc::new(Mutex::new(Vec::new()));
    let (tool, mut handle) = GatedTool::new(impostor, Arc::clone(&order));
    tool.register(&mut tools);
    let impostor_observer = Arc::new(RecordingObserver::new(Vec::new()));
    let impostor_recorded = impostor_observer.recorded();
    let impostor_model = fake_model(tool_turn_then_stop(&[ScriptedCall {
        id: "call-read",
        tool_id: "tool-mcp-read",
        name: "read",
        arguments: serde_json::json!({}),
    }]));
    let releaser = tokio::spawn(async move {
        handle.await_started().await;
        handle.release_and_await_completion().await;
    });
    let impostor_result = run(
        &impostor_model,
        tools,
        ContextAssembly::new(),
        native_lifecycle(impostor_observer),
        &AgentCancellation::new(CancellationReason::UserRequested),
    )
    .await;
    releaser.await.expect("impostor releaser");
    assert!(matches!(
        impostor_result.outcome,
        AttemptOutcome::Completed { .. }
    ));
    let impostor = impostor_recorded.lock().expect("recorded lock").clone();
    assert_eq!(impostor.len(), 1);
    assert_eq!(
        impostor[0].origin,
        ToolOrigin::Mcp {
            server_id: rustx::runtime::identity::McpServerId::new("mcp-fs"),
        }
    );
    assert!(
        !is_native_read(&impostor[0]),
        "a non-native tool named `read` is never the native Read capability"
    );
}

/// An observer failure after structural settlement cannot undo, split, or
/// duplicate the canonical result batch, and it commits no deferred context.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn observer_failure_preserves_the_complete_tool_result_batch() {
    // The failure happens on the *first* observation, so a stranded later
    // sibling would be visible immediately.
    let observer = Arc::new(RecordingObserver::failing("call-a"));
    let recorded = observer.recorded();
    let (result, _physical) = run_inverted_parallel_batch(
        native_lifecycle(observer),
        CancellationReason::UserRequested,
        None,
    )
    .await;

    assert_eq!(
        recorded.lock().expect("recorded lock").len(),
        1,
        "the pass stops at the first failing observation"
    );
    assert!(matches!(
        &result.outcome,
        AttemptOutcome::Failed {
            error: AttemptFailure::Runtime {
                error: RuntimeError::ToolResultObservationFailed { message },
            },
        } if message == "observation of call-a failed"
    ));
    assert_eq!(
        ledger_shape(&result),
        vec![
            "user(Message):go".to_owned(),
            "assistant(call-a,call-b)".to_owned(),
            "tool_result(call-a)".to_owned(),
            "tool_result(call-b)".to_owned(),
        ],
        "the committed Assistant batch keeps its complete canonical result batch"
    );
    let committed = tool_messages(&result);
    assert_eq!(committed.len(), 2, "no second result appears");
    assert_eq!(committed[0].result.status, ToolExecutionStatus::Success);
    assert_eq!(committed[1].result.status, ToolExecutionStatus::Success);
    assert_eq!(
        result.snapshot_history().len(),
        1,
        "no provider request begins after the failed observation"
    );
    assert_single_terminal(&result.event_history);
}

/// Cancellation observed while a bounded observation is pending does not
/// cancel the observation and does not give the observer cancellation
/// ownership: the observation settles, the complete result batch stays
/// canonical, its deferred context is discarded, and the attempt settles
/// cancelled exactly once.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancellation_during_observation_discards_deferred_context() {
    let (entered, entered_rx) = watch::channel(false);
    let (release, release_rx) = watch::channel(false);
    let observer = Arc::new(RecordingObserver::gated(
        vec![("call-a", vec!["deferred context"])],
        entered,
        release_rx,
    ));
    let recorded = observer.recorded();
    let (result, _physical) = run_inverted_parallel_batch(
        native_lifecycle(observer),
        CancellationReason::UserRequested,
        Some(ObservationGate {
            entered: entered_rx,
            release,
        }),
    )
    .await;

    assert!(
        !recorded.lock().expect("recorded lock").is_empty(),
        "the observation actually ran"
    );
    assert!(matches!(
        result.outcome,
        AttemptOutcome::Cancelled {
            reason: CancellationReason::UserRequested
        }
    ));
    assert_eq!(
        ledger_shape(&result),
        vec![
            "user(Message):go".to_owned(),
            "assistant(call-a,call-b)".to_owned(),
            "tool_result(call-a)".to_owned(),
            "tool_result(call-b)".to_owned(),
        ],
        "no deferred context becomes canonical after cancellation"
    );
    assert_eq!(
        result.snapshot_history().len(),
        1,
        "no second model step is admitted"
    );
    assert_single_terminal(&result.event_history);
}

/// Cancellation already observable when the batch settles skips the whole
/// observation phase: there is no useful deferred model context to produce,
/// the result batch is still structurally complete, and the attempt settles
/// cancelled once.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancellation_before_observation_skips_the_observation_phase() {
    let observer = Arc::new(RecordingObserver::new(vec![(
        "call-a",
        vec!["deferred context"],
    )]));
    let recorded = observer.recorded();

    let mut tools = ToolRegistry::new();
    let order = Arc::new(Mutex::new(Vec::new()));
    let (alpha, mut alpha_handle) =
        GatedTool::new(parallel_tool("alpha", "tool-alpha"), Arc::clone(&order));
    let (beta, mut beta_handle) =
        GatedTool::new(parallel_tool("beta", "tool-beta"), Arc::clone(&order));
    alpha.register(&mut tools);
    beta.register(&mut tools);
    let model = fake_model(tool_turn_then_stop(&[
        scripted_call("call-a", "tool-alpha", "alpha"),
        scripted_call("call-b", "tool-beta", "beta"),
    ]));
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let controller_cancellation = cancellation.clone();
    let controller = tokio::spawn(async move {
        alpha_handle.await_started().await;
        beta_handle.await_started().await;
        // Cancellation is observable before either gate opens, so it is
        // observable when the settled batch reaches the observation phase.
        controller_cancellation.cancel();
        beta_handle.release_and_await_completion().await;
        alpha_handle.release_and_await_completion().await;
    });
    let result = run(
        &model,
        tools,
        ContextAssembly::new(),
        native_lifecycle(observer),
        &cancellation,
    )
    .await;
    controller.await.expect("batch controller");

    assert!(
        recorded.lock().expect("recorded lock").is_empty(),
        "no observation runs once cancellation is already observable"
    );
    assert!(matches!(
        result.outcome,
        AttemptOutcome::Cancelled {
            reason: CancellationReason::UserRequested
        }
    ));
    assert_eq!(
        ledger_shape(&result),
        vec![
            "user(Message):go".to_owned(),
            "assistant(call-a,call-b)".to_owned(),
            "tool_result(call-a)".to_owned(),
            "tool_result(call-b)".to_owned(),
        ],
        "the batch is still structurally complete in canonical order"
    );
    assert_single_terminal(&result.event_history);
}

// ---------------------------------------------------------------------------
// Lifecycle timing is not semantic ownership
// ---------------------------------------------------------------------------

/// The accepted `(source, kind, text)` of every admitted context fact, in
/// canonical committed order.
fn committed_context(
    result: &common::DurableExecutionAudit,
) -> Vec<(UserSource, ContextKind, String)> {
    result
        .messages()
        .iter()
        .filter_map(|message| match message {
            MessageBlock::User(user) => match &user.kind {
                InboundKind::Context(kind) => {
                    Some((user.source.clone(), *kind, text_of(&user.content)))
                }
                _ => None,
            },
            _ => None,
        })
        .collect()
}

/// The contributor identities recorded by the frozen context generation of the
/// step that admitted deferred context.
fn deferred_step_contributors(
    result: &common::DurableExecutionAudit,
) -> Vec<ContextContributorIdentity> {
    result.snapshot_history()[1]
        .context_generation
        .contributors
        .iter()
        .map(|entry| entry.identity.clone())
        .collect()
}

/// A deferred proposal produced by the observer registered for the **native**
/// runtime observation owner receives native runtime provenance — because of
/// that registration, not because it was produced after a tool batch.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_observer_deferred_context_receives_native_provenance() {
    let observer = Arc::new(RecordingObserver::new(vec![(
        "call-a",
        vec!["native fact"],
    )]));
    let (result, _physical) = run_inverted_parallel_batch(
        native_lifecycle(observer),
        CancellationReason::UserRequested,
        None,
    )
    .await;

    assert_eq!(
        committed_context(&result),
        vec![(
            UserSource::Runtime,
            ContextKind::RuntimeToolObservation,
            "native fact".to_owned(),
        )],
    );
    assert_eq!(
        deferred_step_contributors(&result),
        vec![ContextContributorIdentity::Native(
            NativeContextContributor::RuntimeToolObservation,
        )],
    );
    assert_single_terminal(&result.event_history);
}

/// A deferred proposal produced by an observer registered for a **certified
/// extension** keeps that extension's provenance, its semantic family, and its
/// contributor identity. Post-tool timing never converts it into native
/// runtime context.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn extension_observer_deferred_context_preserves_extension_provenance() {
    let observer = Arc::new(RecordingObserver::new(vec![(
        "call-a",
        vec!["extension fact"],
    )]));
    let (result, _physical) = run_inverted_parallel_batch_with_assembly(
        extension_lifecycle("example.extension", observer),
        assembly_certifying("example.extension", Some("package-7")),
        CancellationReason::UserRequested,
        None,
    )
    .await;

    assert_eq!(
        committed_context(&result),
        vec![(
            extension_source("example.extension"),
            ContextKind::ExtensionEnvironment,
            "extension fact".to_owned(),
        )],
        "the extension keeps its provenance and its own semantic family"
    );
    assert_eq!(
        deferred_step_contributors(&result),
        vec![ContextContributorIdentity::CertifiedExtension(
            CertifiedExtensionIdentity::new("example.extension").expect("identity"),
        )],
        "post-tool timing does not rewrite the contributor identity"
    );
    assert_eq!(
        result.snapshot_history()[1].context_generation.contributors[0].attestation,
        Some("package-7".to_owned()),
        "the authoritative registered attestation is frozen, not a synthesized one"
    );
    assert!(
        !deferred_step_contributors(&result).contains(&ContextContributorIdentity::Native(
            NativeContextContributor::RuntimeToolObservation,
        )),
        "the native observation owner never appears for an extension's fact"
    );
    assert_single_terminal(&result.event_history);
}

/// Context Assembly registration is the **only** semantic admission authority.
/// Binding a lifecycle observer under an extension key that the attempt's
/// Context Assembly never certified cannot mint extension provenance: the
/// deferred batch is rejected before any context is admitted.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unregistered_extension_observer_cannot_get_extension_provenance() {
    let observer = Arc::new(RecordingObserver::new(vec![(
        "call-a",
        vec!["unauthorized fact"],
    )]));
    let recorded = observer.recorded();
    let (result, _physical) = run_inverted_parallel_batch_with_assembly(
        extension_lifecycle("impostor.extension", observer),
        // A different extension is certified, so the registry is non-empty and
        // the rejection is about identity, not about having no extensions.
        assembly_certifying("known.extension", Some("package-1")),
        CancellationReason::UserRequested,
        None,
    )
    .await;

    assert!(
        !recorded.lock().expect("recorded lock").is_empty(),
        "the observer really ran; the rejection is at admission, not at binding"
    );
    assert!(
        matches!(
            &result.outcome,
            AttemptOutcome::Failed {
                error: AttemptFailure::Runtime { error },
            } if format!("{error:?}").contains("impostor.extension")
        ),
        "unexpected outcome: {:?}",
        result.outcome
    );
    assert!(
        committed_context(&result).is_empty(),
        "no context was admitted, so no extension provenance was ever assigned"
    );
    assert_eq!(
        ledger_shape(&result),
        vec![
            "user(Message):go".to_owned(),
            "assistant(call-a,call-b)".to_owned(),
            "tool_result(call-a)".to_owned(),
            "tool_result(call-b)".to_owned(),
        ],
        "the complete canonical result batch survives the rejection"
    );
    assert_eq!(
        result.snapshot_history().len(),
        1,
        "the rejected step froze no snapshot"
    );
    assert_single_terminal(&result.event_history);
}

/// A **post-tool-only** certified extension — one with no request-time
/// proposals at all — still produces deferred context through its
/// authoritative registration. Certification is what makes it an extension.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_post_tool_only_certified_extension_produces_deferred_context() {
    let observer = Arc::new(RecordingObserver::new(vec![(
        "call-a",
        vec!["post-tool only"],
    )]));
    let (result, _physical) = run_inverted_parallel_batch_with_assembly(
        extension_lifecycle("observer.only", observer),
        // The registered contributor returns nothing at request time.
        assembly_certifying("observer.only", Some("package-3")),
        CancellationReason::UserRequested,
        None,
    )
    .await;

    assert_eq!(
        committed_context(&result),
        vec![(
            extension_source("observer.only"),
            ContextKind::ExtensionEnvironment,
            "post-tool only".to_owned(),
        )],
    );
    assert_eq!(
        result.snapshot_history()[1].context_generation.contributors,
        vec![rustx::context::ContributorGeneration {
            identity: ContextContributorIdentity::CertifiedExtension(
                CertifiedExtensionIdentity::new("observer.only").expect("identity"),
            ),
            attestation: Some("package-3".to_owned()),
        }],
        "a producer that only defers is still explained by its registration"
    );
    assert_single_terminal(&result.event_history);
}

/// The same certified extension produces the same semantic fact whether it
/// contributes at request time or defers after a tool batch: identical
/// provenance, identical family, identical contributor identity. Only the
/// order inside the owner's lane records which describes the preceding batch.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn timing_does_not_change_an_owner_semantics() {
    let observer = Arc::new(RecordingObserver::new(vec![("call-a", vec!["deferred"])]));
    let invocations = Arc::new(AtomicUsize::new(0));
    let mut assembly = ContextAssembly::new();
    assembly
        .register_extension(
            "example.extension",
            Some("package-1".to_owned()),
            contributor("request-time", Arc::clone(&invocations)),
        )
        .expect("register extension");

    let mut tools = ToolRegistry::new();
    InstantTool::register(parallel_tool("alpha", "tool-alpha"), &mut tools);
    let model = fake_model(tool_turn_then_stop(&[scripted_call(
        "call-a",
        "tool-alpha",
        "alpha",
    )]));
    let result = run(
        &model,
        tools,
        assembly,
        extension_lifecycle("example.extension", observer),
        &AgentCancellation::new(CancellationReason::UserRequested),
    )
    .await;

    assert_eq!(
        committed_context(&result),
        vec![
            (
                extension_source("example.extension"),
                ContextKind::ExtensionEnvironment,
                "request-time".to_owned(),
            ),
            (
                extension_source("example.extension"),
                ContextKind::ExtensionEnvironment,
                "deferred".to_owned(),
            ),
            (
                extension_source("example.extension"),
                ContextKind::ExtensionEnvironment,
                "request-time".to_owned(),
            ),
        ],
        "the deferred fact of the second step precedes that step's request-time fact, \
         and both carry the identical owner semantics"
    );
    assert_eq!(
        deferred_step_contributors(&result),
        vec![ContextContributorIdentity::CertifiedExtension(
            CertifiedExtensionIdentity::new("example.extension").expect("identity"),
        )],
        "one owner appears exactly once even when it contributed in both phases"
    );
    assert_single_terminal(&result.event_history);
}

/// Two deferred-context producers with different identities are ordered by
/// their semantic lane and then by logical identity, and the result does not
/// depend on the order in which they were registered.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_producers_keep_deterministic_identity_order() {
    async fn run_with(native_first: bool) -> Vec<(UserSource, ContextKind, String)> {
        let native = Arc::new(RecordingObserver::new(vec![
            ("call-a", vec!["native-a"]),
            ("call-b", vec!["native-b"]),
        ]));
        let extension = Arc::new(RecordingObserver::new(vec![
            ("call-a", vec!["extension-a"]),
            ("call-b", vec!["extension-b"]),
        ]));
        let lifecycle =
            native_and_extension_lifecycle(native, "example.extension", extension, native_first);
        let (result, _physical) = run_inverted_parallel_batch_with_assembly(
            lifecycle,
            assembly_certifying("example.extension", None),
            CancellationReason::UserRequested,
            None,
        )
        .await;
        assert_single_terminal(&result.event_history);
        committed_context(&result)
    }

    let expected = vec![
        (
            UserSource::Runtime,
            ContextKind::RuntimeToolObservation,
            "native-a".to_owned(),
        ),
        (
            UserSource::Runtime,
            ContextKind::RuntimeToolObservation,
            "native-b".to_owned(),
        ),
        (
            extension_source("example.extension"),
            ContextKind::ExtensionEnvironment,
            "extension-a".to_owned(),
        ),
        (
            extension_source("example.extension"),
            ContextKind::ExtensionEnvironment,
            "extension-b".to_owned(),
        ),
    ];
    assert_eq!(
        run_with(true).await,
        expected,
        "each owner keeps canonical batch order inside its own lane"
    );
    assert_eq!(
        run_with(false).await,
        expected,
        "registration order is not observable"
    );
}

/// One semantic owner has one deferred-context producer: a second
/// registration of the same identity is rejected, so no identity can be
/// claimed twice and no ordering ambiguity exists.
#[test]
fn a_semantic_owner_has_at_most_one_observer() {
    let error = AttemptLifecycle::inert()
        .with_native_tool_result_observer(Arc::new(RecordingObserver::new(Vec::new())))
        .expect("native owner")
        .with_native_tool_result_observer(Arc::new(RecordingObserver::new(Vec::new())))
        .expect_err("the native owner is already claimed");
    assert!(error.message.contains("already has a tool-result observer"));

    let identity = CertifiedExtensionIdentity::new("example.extension").expect("identity");
    let error = AttemptLifecycle::inert()
        .with_extension_tool_result_observer(
            identity.clone(),
            Arc::new(RecordingObserver::new(Vec::new())),
        )
        .expect("extension owner")
        .with_extension_tool_result_observer(identity, Arc::new(RecordingObserver::new(Vec::new())))
        .expect_err("the extension is already claimed");
    assert!(error.message.contains("already has a tool-result observer"));
}

// ---------------------------------------------------------------------------
// Immutable invocation facts
// ---------------------------------------------------------------------------

/// A consumer identifies *which file* the native Read capability touched from
/// the validated invocation arguments alone — without re-reading canonical
/// history, parsing Assistant messages, or keeping a duplicate invocation
/// index. Capability recognition still uses only the typed identity: the
/// model-facing name is not part of the observation at all.
#[tokio::test]
async fn observer_reads_the_native_read_target_from_validated_arguments() {
    let fixture = common::native_fixture();
    std::fs::write(
        fixture.runtime.workspace().root().join("notes.txt"),
        "hello\n",
    )
    .expect("workspace file");
    let observer = Arc::new(RecordingObserver::new(Vec::new()));
    let recorded = observer.recorded();
    let model = fake_model(tool_turn_then_stop(&[ScriptedCall {
        id: "call-read",
        tool_id: "tool-read",
        name: "read",
        arguments: serde_json::json!({
            "path": fixture
                .runtime
                .workspace()
                .root()
                .join("notes.txt")
                .to_str()
                .expect("utf8"),
        }),
    }]));
    let capability = common::capability_lease(fixture.registry.clone(), &fixture.runtime).await;
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let result = AgentExecution::new(
        request(fixture.runtime.conversation_id().clone(), &model),
        capability.into_lease(),
        &cancellation,
        context_runtime(&model, ContextAssembly::new()),
        &fixture.runtime,
        native_lifecycle(observer),
    )
    .expect("conversation identity matches the tool runtime")
    .run()
    .await;
    assert!(matches!(result.outcome, AttemptOutcome::Completed { .. }));

    let observations = recorded.lock().expect("recorded lock").clone();
    assert_eq!(observations.len(), 1);
    let invocation = observations[0]
        .invocation
        .as_ref()
        .expect("a preflighted call exposes its validated invocation");
    // Capability recognition: typed identity only.
    assert_eq!(invocation.tool_id, ToolId::new("tool-read"));
    assert_eq!(invocation.origin, ToolOrigin::Builtin);
    assert_eq!(invocation.mode, ToolInvocationMode::Foreground);
    // The fact the result alone under-determines: which file was touched.
    // The validated argument preserves the path spelling the model supplied.
    let expected = fixture
        .runtime
        .workspace()
        .root()
        .join("notes.txt")
        .to_str()
        .expect("utf8")
        .to_owned();
    assert_eq!(
        invocation
            .arguments
            .get("path")
            .and_then(|path| path.as_str()),
        Some(expected.as_str()),
    );
    // The observation carries no model-facing name to compare against, and the
    // arguments carry no reserved invocation metadata.
    assert!(
        invocation
            .arguments
            .as_object()
            .expect("business arguments are an object")
            .keys()
            .all(|key| !key.starts_with("__rustx")),
        "reserved invocation metadata was stripped before the observation"
    );
    assert_eq!(&observations[0].tool_id, &invocation.tool_id);
    assert_eq!(&observations[0].origin, &invocation.origin);
}

/// A call rejected by preflight never resolved an invocation, so it exposes no
/// invocation arguments at all. Its canonical identity is still the registry's
/// resolution, so a consumer can still say *which capability* was refused.
#[tokio::test]
async fn a_preflight_rejected_call_exposes_no_invocation_arguments() {
    let fixture = common::native_fixture();
    let observer = Arc::new(RecordingObserver::new(Vec::new()));
    let recorded = observer.recorded();
    // `path` is required by the canonical Read schema: this call is rejected
    // before invocation resolution and its result slot is the deterministic
    // rejection.
    let model = fake_model(tool_turn_then_stop(&[ScriptedCall {
        id: "call-read",
        tool_id: "tool-read",
        name: "read",
        arguments: serde_json::json!({}),
    }]));
    let capability = common::capability_lease(fixture.registry.clone(), &fixture.runtime).await;
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let result = AgentExecution::new(
        request(fixture.runtime.conversation_id().clone(), &model),
        capability.into_lease(),
        &cancellation,
        context_runtime(&model, ContextAssembly::new()),
        &fixture.runtime,
        native_lifecycle(observer),
    )
    .expect("conversation identity matches the tool runtime")
    .run()
    .await;
    assert!(matches!(result.outcome, AttemptOutcome::Completed { .. }));

    let observations = recorded.lock().expect("recorded lock").clone();
    assert_eq!(observations.len(), 1);
    assert!(
        observations[0].invocation.is_none(),
        "no invocation was ever validated, so none is exposed"
    );
    assert_eq!(
        observations[0].tool_id,
        ToolId::new("tool-read"),
        "the registry-resolved capability identity survives the rejection"
    );
    assert_eq!(observations[0].origin, ToolOrigin::Builtin);
    assert!(matches!(
        observations[0].result.status,
        ToolExecutionStatus::Failed { .. }
    ));
    assert_eq!(tool_messages(&result).len(), 1);
}

// ---------------------------------------------------------------------------
// Cancellation precedence across observers
// ---------------------------------------------------------------------------

/// The linearization this section pins down, with two bound producers over one
/// settled batch:
///
/// ```text
/// cancellation check      ← an observer never starts after this
/// await observer
/// cancellation check      ← wins over the observer's Ok *and* its Err
/// consume result, validate, stage
/// ```
///
/// Observable cancellation must win **before** a later observer starts. The
/// in-flight observation is allowed to settle — it is never dropped just to
/// implement the rule — but the next producer never runs.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancellation_prevents_a_later_observer_from_starting() {
    let (entered, entered_rx) = watch::channel(false);
    let (release, release_rx) = watch::channel(false);
    // The native producer sorts first, so it observes `call-a` before the
    // extension producer gets its turn.
    let native = Arc::new(RecordingObserver::gated(
        vec![("call-a", vec!["staged before cancellation"])],
        entered,
        release_rx,
    ));
    let native_recorded = native.recorded();
    let extension = Arc::new(RecordingObserver::new(vec![(
        "call-a",
        vec!["must never run"],
    )]));
    let extension_recorded = extension.recorded();

    let (result, _physical) = run_inverted_parallel_batch_with_assembly(
        native_and_extension_lifecycle(native, "example.extension", extension, true),
        assembly_certifying("example.extension", None),
        CancellationReason::UserRequested,
        Some(ObservationGate {
            entered: entered_rx,
            release,
        }),
    )
    .await;

    assert_eq!(
        native_recorded.lock().expect("recorded lock").len(),
        1,
        "the in-flight observation settled rather than being dropped"
    );
    assert!(
        extension_recorded.lock().expect("recorded lock").is_empty(),
        "no later observer starts once cancellation is observable"
    );
    assert!(matches!(
        result.outcome,
        AttemptOutcome::Cancelled {
            reason: CancellationReason::UserRequested
        }
    ));
    assert!(
        committed_context(&result).is_empty(),
        "the settled observer's proposals never became canonical"
    );
    assert_eq!(
        ledger_shape(&result),
        vec![
            "user(Message):go".to_owned(),
            "assistant(call-a,call-b)".to_owned(),
            "tool_result(call-a)".to_owned(),
            "tool_result(call-b)".to_owned(),
        ],
        "the complete canonical result batch is untouched"
    );
    assert_eq!(result.snapshot_history().len(), 1);
    assert_single_terminal(&result.event_history);
}

/// Already-observable cancellation outranks an observer **failure**. An
/// observer that errors while the attempt is already cancelled cannot convert
/// cancellation into `ToolResultObservationFailed`, and the next producer
/// still never starts.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancellation_outranks_an_observer_error() {
    let (entered, entered_rx) = watch::channel(false);
    let (release, release_rx) = watch::channel(false);
    let native = Arc::new(RecordingObserver::gated_failing(
        "call-a", entered, release_rx,
    ));
    let native_recorded = native.recorded();
    let extension = Arc::new(RecordingObserver::new(Vec::new()));
    let extension_recorded = extension.recorded();

    let (result, _physical) = run_inverted_parallel_batch_with_assembly(
        native_and_extension_lifecycle(native, "example.extension", extension, true),
        assembly_certifying("example.extension", None),
        CancellationReason::UserRequested,
        Some(ObservationGate {
            entered: entered_rx,
            release,
        }),
    )
    .await;

    assert_eq!(
        native_recorded.lock().expect("recorded lock").len(),
        1,
        "the failing observation ran to completion"
    );
    assert!(
        extension_recorded.lock().expect("recorded lock").is_empty(),
        "no later observer starts once cancellation is observable"
    );
    assert!(
        matches!(
            result.outcome,
            AttemptOutcome::Cancelled {
                reason: CancellationReason::UserRequested
            }
        ),
        "unexpected outcome: {:?}",
        result.outcome
    );
    assert!(
        !matches!(
            &result.outcome,
            AttemptOutcome::Failed {
                error: AttemptFailure::Runtime {
                    error: RuntimeError::ToolResultObservationFailed { .. },
                },
            }
        ),
        "an observer error never overrides already-observable cancellation"
    );
    assert_single_terminal(&result.event_history);
}

/// Cancellation observed while a *later* producer is pending discards what the
/// *earlier* producer already proposed. Pass-local proposals are never visible
/// to the attempt, so a cancelled pass leaves no deferred state at all.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancellation_discards_the_earlier_observers_proposals() {
    let (entered, entered_rx) = watch::channel(false);
    let (release, release_rx) = watch::channel(false);
    // The native producer completes normally and proposes; the extension
    // producer then parks, and cancellation happens while it is pending.
    let native = Arc::new(RecordingObserver::new(vec![(
        "call-a",
        vec!["proposed before the cancellation"],
    )]));
    let native_recorded = native.recorded();
    let extension = Arc::new(RecordingObserver::gated(
        vec![("call-a", vec!["also discarded"])],
        entered,
        release_rx,
    ));
    let extension_recorded = extension.recorded();

    let (result, _physical) = run_inverted_parallel_batch_with_assembly(
        native_and_extension_lifecycle(native, "example.extension", extension, true),
        assembly_certifying("example.extension", None),
        CancellationReason::UserRequested,
        Some(ObservationGate {
            entered: entered_rx,
            release,
        }),
    )
    .await;

    assert_eq!(
        native_recorded.lock().expect("recorded lock").len(),
        1,
        "the earlier producer observed and proposed"
    );
    assert_eq!(
        extension_recorded.lock().expect("recorded lock").len(),
        1,
        "the later producer was in flight when cancellation became observable"
    );
    assert!(matches!(
        result.outcome,
        AttemptOutcome::Cancelled {
            reason: CancellationReason::UserRequested
        }
    ));
    assert!(
        committed_context(&result).is_empty(),
        "the earlier producer's proposals are discarded with the rest of the pass"
    );
    assert_eq!(
        result.snapshot_history().len(),
        1,
        "no later step could observe a partially staged buffer"
    );
    assert_single_terminal(&result.event_history);
}

/// The deferred seam carries User context only, so an admitted deferred batch
/// can never change the Effective System Prompt of the following request. The
/// restriction is a type — `ToolResultObserver` returns `UserMessageProposal`,
/// so a deferred system section is unrepresentable — and this regression pins
/// the observable consequence end to end.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn deferred_context_never_changes_the_effective_system_prompt() {
    let observer = Arc::new(RecordingObserver::new(vec![(
        "call-a",
        vec!["deferred user fact"],
    )]));
    let (result, _physical) = run_inverted_parallel_batch(
        native_lifecycle(observer),
        CancellationReason::UserRequested,
        None,
    )
    .await;

    assert_eq!(
        committed_context(&result),
        vec![(
            UserSource::Runtime,
            ContextKind::RuntimeToolObservation,
            "deferred user fact".to_owned(),
        )],
        "the deferred fact is admitted as conversational User context"
    );
    let prompts = result
        .snapshot_history()
        .iter()
        .map(|snapshot| snapshot.effective_system_prompt.clone())
        .collect::<Vec<_>>();
    assert_eq!(prompts.len(), 2, "two admitted model steps");
    assert_eq!(
        prompts[0], prompts[1],
        "the step that admitted deferred context has the same Effective System Prompt"
    );
    assert!(
        !prompts[1].contains("deferred user fact"),
        "no deferred text reaches the Effective System Prompt"
    );
    assert_single_terminal(&result.event_history);
}
// ---------------------------------------------------------------------------
// The bounded observer transaction boundary
// ---------------------------------------------------------------------------

/// One observation above the per-observation bound is rejected at the
/// transaction boundary. Nothing of the pass is staged, the complete canonical
/// result batch survives, and the attempt settles once.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_single_observation_above_the_bound_stages_nothing() {
    let observer = Arc::new(BulkObserver::new(MAX_PROPOSALS_PER_CONTRIBUTOR + 1));
    let observations = observer.observations();
    let (result, _physical) = run_inverted_parallel_batch(
        native_lifecycle(observer),
        CancellationReason::UserRequested,
        None,
    )
    .await;

    assert_eq!(
        observations.load(Ordering::SeqCst),
        1,
        "the pass stops at the first observation that violates the bound"
    );
    assert!(
        matches!(
            &result.outcome,
            AttemptOutcome::Failed {
                error: AttemptFailure::Runtime {
                    error: RuntimeError::DeferredContextRejected { message },
                },
            } if message.contains("above the bounded proposal limit")
        ),
        "unexpected outcome: {:?}",
        result.outcome
    );
    assert_eq!(
        ledger_shape(&result),
        vec![
            "user(Message):go".to_owned(),
            "assistant(call-a,call-b)".to_owned(),
            "tool_result(call-a)".to_owned(),
            "tool_result(call-b)".to_owned(),
        ],
        "no deferred context became canonical and the result batch is complete"
    );
    assert_eq!(
        result.snapshot_history().len(),
        1,
        "no provider request begins after the rejected pass"
    );
    assert_single_terminal(&result.event_history);
}

/// Individually bounded observations that together exceed the aggregate
/// deferred-context bound are rejected at the transaction boundary of the
/// observation that would cross it, before the attempt buffer is touched.
#[tokio::test]
async fn observations_that_together_exceed_the_aggregate_bound_stage_nothing() {
    let per_call = MAX_PROPOSALS_PER_CONTRIBUTOR;
    // The first call whose staging would cross the aggregate bound.
    let crossing_call = MAX_DEFERRED_CONTEXT_PROPOSALS / per_call;
    let calls = crossing_call + 1;
    let mut tools = ToolRegistry::new();
    InstantTool::register(parallel_tool("alpha", "tool-alpha"), &mut tools);
    let scripted = (0..calls)
        .map(|index| scripted_call(&format!("call-{index}"), "tool-alpha", "alpha"))
        .collect::<Vec<_>>();
    let model = fake_model(tool_turn_then_stop(&scripted));
    let observer = Arc::new(BulkObserver::new(per_call));
    let observations = observer.observations();
    let result = run(
        &model,
        tools,
        ContextAssembly::new(),
        native_lifecycle(observer),
        &AgentCancellation::new(CancellationReason::UserRequested),
    )
    .await;

    assert!(
        per_call * crossing_call <= MAX_DEFERRED_CONTEXT_PROPOSALS
            && per_call * calls > MAX_DEFERRED_CONTEXT_PROPOSALS,
        "the fixture actually straddles the aggregate bound"
    );
    assert_eq!(
        observations.load(Ordering::SeqCst),
        calls,
        "every individually bounded observation ran; the aggregate crossed on the last"
    );
    assert!(
        matches!(
            &result.outcome,
            AttemptOutcome::Failed {
                error: AttemptFailure::Runtime {
                    error: RuntimeError::DeferredContextRejected { message },
                },
            } if message.contains("deferred proposals")
        ),
        "unexpected outcome: {:?}",
        result.outcome
    );
    assert!(
        committed_context(&result).is_empty(),
        "not one proposal of the rejected pass became canonical"
    );
    assert_eq!(
        tool_messages(&result).len(),
        calls,
        "the complete canonical result batch survives the rejected pass"
    );
    assert_eq!(result.snapshot_history().len(), 1);
    assert_single_terminal(&result.event_history);
}

/// A failing observation discards the proposals the *earlier* observations of
/// the same pass already produced: the pass is one transaction, so it leaves
/// no partial deferred state behind.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failed_observation_leaves_no_deferred_context() {
    // `call-a` succeeds and proposes; `call-b` then fails.
    let mut observer = RecordingObserver::new(vec![("call-a", vec!["staged before the failure"])]);
    observer.fail_on = Some("call-b");
    let observer = Arc::new(observer);
    let recorded = observer.recorded();
    let (result, _physical) = run_inverted_parallel_batch(
        native_lifecycle(observer),
        CancellationReason::UserRequested,
        None,
    )
    .await;

    assert_eq!(
        recorded.lock().expect("recorded lock").len(),
        2,
        "the earlier observation ran and proposed before the later one failed"
    );
    assert!(matches!(
        &result.outcome,
        AttemptOutcome::Failed {
            error: AttemptFailure::Runtime {
                error: RuntimeError::ToolResultObservationFailed { message },
            },
        } if message == "observation of call-b failed"
    ));
    assert!(
        committed_context(&result).is_empty(),
        "the successful earlier proposal of the failed pass is discarded too"
    );
    assert_eq!(
        ledger_shape(&result),
        vec![
            "user(Message):go".to_owned(),
            "assistant(call-a,call-b)".to_owned(),
            "tool_result(call-a)".to_owned(),
            "tool_result(call-b)".to_owned(),
        ],
    );
    assert_eq!(result.snapshot_history().len(), 1);
    assert_single_terminal(&result.event_history);
}

// ---------------------------------------------------------------------------
// The shared inverted-completion batch driver
// ---------------------------------------------------------------------------

/// The gate a caller uses to make cancellation observable exactly while the
/// tool-result observation is parked.
struct ObservationGate {
    entered: watch::Receiver<bool>,
    release: watch::Sender<bool>,
}

/// Runs one attempt whose Assistant message carries two parallel-capable
/// calls, `call-a` (alpha) and `call-b` (beta), and forces `beta` to
/// physically complete before `alpha`.
///
/// The inversion is exact rather than probabilistic: the controller waits
/// until both executions started, releases only `beta`, awaits `beta`'s
/// recorded completion while `alpha` is still parked on its own gate, and
/// only then releases `alpha`. The returned physical order is therefore
/// always `[beta, alpha]`.
async fn run_inverted_parallel_batch(
    lifecycle: AttemptLifecycle,
    reason: CancellationReason,
    observation_gate: Option<ObservationGate>,
) -> (common::DurableExecutionAudit, Vec<String>) {
    run_inverted_parallel_batch_with_assembly(
        lifecycle,
        ContextAssembly::new(),
        reason,
        observation_gate,
    )
    .await
}

/// [`run_inverted_parallel_batch`] over an explicit Context Assembly, so a test
/// can control which extensions are certified for the attempt.
async fn run_inverted_parallel_batch_with_assembly(
    lifecycle: AttemptLifecycle,
    assembly: ContextAssembly,
    reason: CancellationReason,
    observation_gate: Option<ObservationGate>,
) -> (common::DurableExecutionAudit, Vec<String>) {
    let mut tools = ToolRegistry::new();
    let order = Arc::new(Mutex::new(Vec::new()));
    let (alpha, mut alpha_handle) =
        GatedTool::new(parallel_tool("alpha", "tool-alpha"), Arc::clone(&order));
    let (beta, mut beta_handle) =
        GatedTool::new(parallel_tool("beta", "tool-beta"), Arc::clone(&order));
    alpha.register(&mut tools);
    beta.register(&mut tools);
    let model = fake_model(tool_turn_then_stop(&[
        scripted_call("call-a", "tool-alpha", "alpha"),
        scripted_call("call-b", "tool-beta", "beta"),
    ]));
    let cancellation = AgentCancellation::new(reason);
    let order_for_controller = Arc::clone(&order);
    let controller = tokio::spawn(async move {
        alpha_handle.await_started().await;
        beta_handle.await_started().await;
        beta_handle.release_and_await_completion().await;
        assert_eq!(
            order_for_controller
                .lock()
                .expect("completion order lock")
                .as_slice(),
            ["beta".to_owned()],
            "A is still parked on its own gate when B has already completed"
        );
        alpha_handle.release_and_await_completion().await;
    });
    let observation_controller = observation_gate.map(|gate| {
        let cancellation = cancellation.clone();
        tokio::spawn(async move {
            let ObservationGate {
                mut entered,
                release,
            } = gate;
            entered
                .wait_for(|entered| *entered)
                .await
                .expect("observation entered");
            cancellation.cancel();
            release.send_replace(true);
        })
    });
    let result = run(&model, tools, assembly, lifecycle, &cancellation).await;
    controller.await.expect("batch controller");
    if let Some(handle) = observation_controller {
        handle.await.expect("observation controller");
    }
    let physical = order.lock().expect("completion order lock").clone();
    (result, physical)
}
