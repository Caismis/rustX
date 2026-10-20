//! Scripted execution fixtures shared by the execution test modules.

use crate::agent::execution::AgentExecutionRequest;
use crate::agent::observer::AgentExecutionObserver;
use crate::context::ContextRuntime;
use crate::conversation::ConversationState;
use crate::durable::inbox::ConversationStore;
use crate::events::types::RuntimeEvent;
use crate::message::types::{ContentBlockIndex, MessageBlock};
use crate::model::adapter::{ModelAdapter, ModelStream, ModelStreamItem};
use crate::model::chat_protocol;
use crate::model::error::ModelErrorKind;
use crate::model::event::ModelEvent;
use crate::model::finish::ModelFinishReason;
use crate::model::types::{ModelProtocol, ModelRequest};
use crate::publication::{PublicationFrame, PublicationStreamStart};
use crate::runtime::cancellation::CancellationSignal;
use crate::runtime::identity::{AgentId, AttemptId, ConversationId, ToolCallId, ToolId};
use crate::runtime::inbound::InitialTurnTrigger;
use crate::scripted_suites::support::model::scripted_session_model;
use crate::tools::executor::ToolExecutor;
use crate::tools::types::{
    ToolCall, ToolCallStart, ToolConcurrencyPolicy, ToolDefinition, ToolExecutionPolicy,
    ToolExecutionResult, ToolExecutionStatus, ToolInvocation, ToolOrigin, ToolReplayPolicy,
};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

/// A scripted model adapter: each invocation pops the next event script
/// and yields it synchronously, recording every request.
pub(super) struct ScriptedAdapter {
    pub(super) scripts: Mutex<VecDeque<Vec<ModelEvent>>>,
    pub(super) requests: Arc<Mutex<Vec<ModelRequest>>>,
}

impl ScriptedAdapter {
    pub(super) fn new(scripts: Vec<Vec<ModelEvent>>) -> Self {
        Self {
            scripts: Mutex::new(scripts.into()),
            requests: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub(super) fn request_count(&self) -> usize {
        self.requests
            .lock()
            .expect("scripted adapter request lock")
            .len()
    }

    pub(super) fn requests(&self) -> Vec<ModelRequest> {
        self.requests
            .lock()
            .expect("scripted adapter request lock")
            .clone()
    }
}

impl ModelAdapter for ScriptedAdapter {
    fn protocol(&self) -> ModelProtocol {
        chat_protocol()
    }

    fn stream(&self, request: ModelRequest, _cancellation: CancellationSignal) -> ModelStream {
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
        Box::pin(futures_util::stream::iter(
            script.into_iter().map(ModelStreamItem::Event),
        ))
    }
}

#[derive(Default)]
pub(super) struct RecordingObserver {
    pub(super) start_gap: Option<(
        Arc<crate::runtime::monotonic::ManualMonotonicClock>,
        Arc<crate::durable::SqliteConversationStore>,
    )>,
    pub(super) events: Mutex<Vec<RuntimeEvent>>,
    pub(super) committed: Mutex<Vec<MessageBlock>>,
    pub(super) statuses: Mutex<Vec<crate::agent::AgentStatusObservation>>,
    /// The released publication frames, in release order. Every frame
    /// here is already durably committed for release.
    pub(super) frames: Mutex<Vec<PublicationFrame>>,
    /// The audits of every stream that settled without canonical
    /// acceptance.
    pub(super) audits: Mutex<Vec<crate::publication::PublicationAudit>>,
    /// The live (not yet durable) foreground tool progress reports, in
    /// observation order (Issue #178).
    pub(super) tool_progress: Mutex<Vec<(ToolCallId, crate::tools::types::ToolProgress)>>,
}

impl RecordingObserver {
    pub(super) fn events(&self) -> Vec<RuntimeEvent> {
        self.events.lock().expect("observer event lock").clone()
    }

    pub(super) fn tool_progress(&self) -> Vec<(ToolCallId, crate::tools::types::ToolProgress)> {
        self.tool_progress
            .lock()
            .expect("observer tool progress lock")
            .clone()
    }
}

impl AgentExecutionObserver for RecordingObserver {
    fn observe_event(
        &self,
        _attempt_id: &AttemptId,
        event: &RuntimeEvent,
        _journal_sequence: u64,
        _tool_occurrence: Option<&crate::events::types::ToolObservationOccurrence>,
        _receipt: Option<&crate::durable::observation::NativeObservationBatch>,
    ) {
        if matches!(event, RuntimeEvent::ModelRequestStarted { .. })
            && let Some((clock, store)) = &self.start_gap
        {
            assert!(
                event_history(store.as_ref())
                    .iter()
                    .any(|persisted| persisted == event)
            );
            assert_eq!(
                crate::runtime::monotonic::MonotonicClock::now_millis(clock.as_ref()),
                0
            );
            clock.advance(400);
        }
        self.events
            .lock()
            .expect("observer event lock")
            .push(event.clone());
    }

    fn observe_committed(
        &self,
        _attempt_id: &AttemptId,
        block: &MessageBlock,
        _receipt: &crate::durable::inbox::TranscriptCommitReceipt,
    ) {
        self.committed
            .lock()
            .expect("observer committed lock")
            .push(block.clone());
    }

    fn observe_status(
        &self,
        observation: &crate::agent::AgentStatusObservation,
        _source: Option<&crate::durable::inbox::StatusObservationSource>,
    ) {
        self.statuses
            .lock()
            .expect("observer status lock")
            .push(observation.clone());
    }

    fn observe_publication_opened(&self, _attempt_id: &AttemptId, _start: &PublicationStreamStart) {
    }

    fn observe_publication(
        &self,
        _attempt_id: &AttemptId,
        frame: &PublicationFrame,
        _receipt: &crate::publication::PublicationFrameReceipt,
    ) {
        self.frames
            .lock()
            .expect("observer frame lock")
            .push(frame.clone());
    }

    fn observe_publication_settled(
        &self,
        _attempt_id: &AttemptId,
        audit: &crate::publication::PublicationAudit,
        _receipt: &crate::durable::inbox::PublicationAuditReceipt,
    ) {
        self.audits
            .lock()
            .expect("observer audit lock")
            .push(audit.clone());
    }

    fn observe_tool_progress(
        &self,
        _attempt_id: &AttemptId,
        tool_call_id: &ToolCallId,
        _tool_id: &ToolId,
        progress: &crate::tools::types::ToolProgress,
    ) {
        self.tool_progress
            .lock()
            .expect("observer tool progress lock")
            .push((tool_call_id.clone(), progress.clone()));
    }
}

/// An instant fake executor returning one fixed successful result.
pub(super) struct InstantTool;

impl InstantTool {
    pub(super) fn definition(id: &str, name: &str) -> ToolDefinition {
        ToolDefinition {
            id: ToolId::new(id),
            name: name.to_owned(),
            description: String::new(),
            input_schema: serde_json::json!({"type": "object"}),
            execution_policy: ToolExecutionPolicy::ForegroundOnly,
            concurrency_policy: ToolConcurrencyPolicy::Sequential,
            approval_policy: crate::tools::types::ToolApprovalPolicy::Never,
            replay_policy: ToolReplayPolicy::Never,
            origin: ToolOrigin::Builtin,
        }
    }
}

impl ToolExecutor for InstantTool {
    fn start<'a>(
        &'a self,
        _invocation: ToolInvocation,
        context: crate::tools::executor::ToolExecutionContext<'a>,
    ) -> crate::tools::executor::ToolExecutionHandle<'a> {
        crate::tools::executor::ToolExecutionHandle::settled_by_operation(
            Box::pin(async {
                ToolExecutionResult {
                    deliveries: Vec::new(),
                    status: ToolExecutionStatus::Success,
                    content: Vec::new(),
                    duration_ms: 0,
                    exit_code: None,
                    artifacts: Vec::new(),
                    truncation: None,
                    workflow: None,
                    managed_output: None,
                }
            }),
            context.cancellation.clone(),
        )
    }

    fn progress_capability(&self) -> crate::tools::deadline::ToolProgressCapability {
        crate::tools::deadline::ToolProgressCapability::None
    }
}

/// An adapter whose stream parks until the invocation's cancellation
/// signal fires, then fails with `Cancelled` — the execution-level
/// analogue of the scripted suites' `FakeStep::ParkUntilCancelled`.
#[derive(Default)]
pub(super) struct ParkedUntilCancelledAdapter {
    pub(super) requests: Mutex<Vec<ModelRequest>>,
}

impl ParkedUntilCancelledAdapter {
    pub(super) fn request_count(&self) -> usize {
        self.requests.lock().expect("request lock").len()
    }
}

impl ModelAdapter for ParkedUntilCancelledAdapter {
    fn protocol(&self) -> ModelProtocol {
        chat_protocol()
    }

    fn stream(&self, request: ModelRequest, cancellation: CancellationSignal) -> ModelStream {
        self.requests.lock().expect("request lock").push(request);
        Box::pin(futures_util::stream::once(async move {
            cancellation.cancelled().await;
            ModelStreamItem::Event(ModelEvent::Failed {
                error: crate::model::error::ModelError {
                    kind: ModelErrorKind::Cancelled,
                    message: "cancelled".to_owned(),
                    retry_disposition: crate::model::error::ModelRetryDisposition::Never,
                    retry_after_ms: None,
                    provider_code: None,
                    context_overflow: None,
                    malformed_tool_proposal: None,
                    timeout_phase: None,
                    generation: None,
                },
            })
        }))
    }
}

/// One attempt request bound to a scripted adapter through a real
/// catalog resolution: the binding still requires an explicit endpoint
/// and credential, exactly as in production.
pub(super) fn request<A: ModelAdapter + 'static>(adapter: &Arc<A>) -> AgentExecutionRequest {
    let adapter: Arc<dyn ModelAdapter> = adapter.clone();
    AgentExecutionRequest {
        agent_id: AgentId::new("agent-a"),
        conversation_id: ConversationId::new("conv_36524fd8-f674-7fc2-8125-06d01fee0e18"),
        attempt_id: AttemptId::new("attempt-1"),
        conversation: ConversationState::new(),
        initial_turn_trigger: InitialTurnTrigger::Continuation,
        model: scripted_session_model(adapter).snapshot().unwrap(),
    }
}

/// A deterministic context runtime derived from the same immutable model
/// snapshot as the execution. The window is far larger than any scripted
/// request, so no compaction ever triggers in these tests.
pub(super) fn runtime<A: ModelAdapter + 'static>(adapter: &Arc<A>) -> ContextRuntime {
    let mut context = ContextRuntime::for_attempt(
        crate::context::SessionContextPolicy {
            reserve_tokens: 0,
            keep_recent_tokens: 0,
            summary_output_cap: None,
        },
        Arc::new(crate::context::DefaultTokenEstimator),
        Some(crate::context::AgentStatusEngine::default()),
        &request(adapter).model,
        crate::model::ModelTimeoutPolicy::default(),
        crate::scripted_suites::support::default_monotonic_clock(),
    )
    .expect("valid context runtime");
    // Boundary fixtures intentionally have no canonical history. Supply a
    // real trusted System input so adapter preflight can admit the request.
    context.native_system.core_runtime_identity =
        Some("Execute the scripted boundary fixture.".into());
    context
}

/// One turn of a single tool call, scripted as canonical events.
pub(super) fn tool_call_script(call: &ToolCall) -> Vec<ModelEvent> {
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

/// Builds one deterministic Workflow terminal-protocol model turn. The
/// helper uses the real normalized model-event assembler, so the tests
/// exercise the same call batching and argument assembly as a provider
/// adapter rather than calling `complete_turn` with an artificial value.
pub(super) fn workflow_output_turn(calls: &[ToolCall], text: Option<&str>) -> Vec<ModelEvent> {
    let mut events = vec![ModelEvent::Started];
    if let Some(text) = text {
        events.push(ModelEvent::TextDelta {
            block_index: ContentBlockIndex::new(0),
            text: text.to_owned(),
        });
    }
    let block_offset = usize::from(text.is_some());
    for (index, call) in calls.iter().enumerate() {
        let block_index = ContentBlockIndex::new(
            u32::try_from(block_offset + index).expect("test call count fits block index"),
        );
        events.push(ModelEvent::ToolCallStarted {
            block_index,
            call: ToolCallStart {
                id: call.id.clone(),
                tool_id: call.tool_id.clone(),
                name: call.name.clone(),
            },
        });
        events.push(ModelEvent::ToolCallArgumentsDelta {
            block_index,
            call_id: call.id.clone(),
            arguments_delta: serde_json::to_string(&call.arguments)
                .expect("workflow arguments are JSON"),
        });
        events.push(ModelEvent::ToolCallCompleted {
            block_index,
            call: call.clone(),
        });
    }
    events.push(ModelEvent::Completed {
        finish_reason: if calls.is_empty() {
            ModelFinishReason::Stop
        } else {
            ModelFinishReason::ToolCalls
        },
        usage: None,
    });
    events
}

/// Reads committed Event Journal facts through bounded pages for tests
/// that explicitly audit the complete attempt history.
pub(super) fn event_history(store: &dyn ConversationStore) -> Vec<RuntimeEvent> {
    const PAGE_SIZE: usize = 32;
    let mut cursor = None;
    let mut events = Vec::new();
    loop {
        let page = store.read_events(cursor, PAGE_SIZE).expect("event page");
        if page.events.is_empty() {
            break;
        }
        // Recorded traces compare semantics. Generation timing evidence
        // is a measurement of one physical generation, so it is
        // normalized out here and asserted by the timing regressions.
        events.extend(
            page.events
                .into_iter()
                .map(|envelope| envelope.event.without_generation_evidence()),
        );
        cursor = page.next_sequence;
    }
    events
}

/// Reads retained Request Snapshots through bounded pages for tests that
/// explicitly audit historical request facts.
pub(super) fn request_snapshot_history(
    store: &dyn ConversationStore,
) -> Vec<crate::model::RequestSnapshot> {
    const PAGE_SIZE: usize = 32;
    let mut cursor = None;
    let mut snapshots = Vec::new();
    loop {
        let page = store
            .read_request_snapshots(cursor, PAGE_SIZE)
            .expect("request snapshot page");
        if page.snapshots.is_empty() {
            break;
        }
        snapshots.extend(page.snapshots);
        cursor = page.next_sequence;
    }
    snapshots
}
