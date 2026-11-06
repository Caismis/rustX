//! Scripted execution fixtures shared by the execution test modules.

use crate::agent::execution::AgentExecutionRequest;
use crate::agent::observer::AgentExecutionObserver;
use crate::agent::{AgentCancellation, AgentExecution};
use crate::context::ContextRuntime;
use crate::conversation::ConversationState;
use crate::durable::ConversationStore;
#[cfg(test)]
use crate::durable::{CanonicalStore, JournalStore, PublicationStore};
use crate::events::AttemptOutcome;
use crate::events::types::RuntimeEvent;
use crate::message::types::{ContentBlockIndex, MessageBlock};
use crate::message::types::{InboundKind, UserContentBlock, UserMessageBlock, UserSource};
use crate::model::adapter::{ModelAdapter, ModelStream, ModelStreamItem};
use crate::model::chat_protocol;
use crate::model::error::ModelErrorKind;
use crate::model::event::ModelEvent;
use crate::model::finish::ModelFinishReason;
use crate::model::types::{ModelProtocol, ModelRequest};
use crate::publication::{PublicationFrame, PublicationStreamStart};
use crate::runtime::MessageId;
use crate::runtime::cancellation::CancellationSignal;
use crate::runtime::identity::{AgentId, AttemptId, ConversationId, ToolCallId, ToolId};
use crate::runtime::inbound::InitialTurnTrigger;
use crate::runtime::types::CancellationReason;
use crate::scripted_suites::common::{capability_lease, tool_runtime_with_store};
use crate::scripted_suites::support::model::scripted_session_model;
use crate::tools::executor::ToolExecutor;
use crate::tools::executor::ToolRegistry;
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

pub(super) struct FinalPublicationProbe {
    mailbox: crate::runtime::inbound::ConversationInboundMailbox,
    terminal: std::sync::atomic::AtomicBool,
    producer: Mutex<Option<std::thread::JoinHandle<()>>>,
}

/// Cancellation at the first post-commit notification cannot reclassify
/// the already committed empty-inbox completion winner.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[allow(clippy::too_many_lines)] // Complete execution fixture and exact publication oracle.
async fn cancellation_after_atomic_final_commit_preserves_completed_winner() {
    let adapter = Arc::new(ScriptedAdapter::new(vec![vec![
        ModelEvent::Started,
        ModelEvent::TextDelta {
            block_index: ContentBlockIndex::new(0),
            text: "done".to_owned(),
        },
        ModelEvent::Completed {
            finish_reason: ModelFinishReason::Stop,
            usage: None,
        },
    ]]));
    let store = Arc::new(
        crate::durable::SqliteConversationStore::in_memory(ConversationId::new(
            "conv_36524fd8-f674-7fc2-8125-06d01fee0e18",
        ))
        .expect("in-memory store"),
    );
    let tool_runtime = tool_runtime_with_store(
        "conv_36524fd8-f674-7fc2-8125-06d01fee0e18",
        Some(store.clone()),
    );
    let (lease, _coordinator) = capability_lease(ToolRegistry::new(), &tool_runtime)
        .await
        .into_lease_and_coordinator();
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let probe = Arc::new(FinalPublicationProbe {
        mailbox: tool_runtime.mailbox(),
        terminal: std::sync::atomic::AtomicBool::new(false),
        producer: Mutex::new(None),
    });
    let observer = RecordingObserver {
        cancel_after_completion: Some(cancellation.clone()),
        final_publication_probe: Some(Arc::clone(&probe)),
        ..Default::default()
    };
    let mut execution = AgentExecution::new(
        request(&adapter),
        lease,
        &cancellation,
        crate::scripted_suites::support::default_execution_policy(),
        runtime(&adapter),
        &tool_runtime,
        crate::agent::AttemptLifecycle::inert(),
    )
    .expect("execution construction");
    execution.observe(&observer);

    let result = execution.run().await;
    probe
        .producer
        .lock()
        .unwrap()
        .take()
        .expect("concurrent producer")
        .join()
        .unwrap();
    let pending = probe
        .mailbox
        .with_pending_snapshot(|items| items)
        .unwrap()
        .expect("publication lease released");
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].message().id.as_str(), "late-after-final");
    assert_eq!(
        adapter.request_count(),
        1,
        "later input did not reopen the committed winner"
    );
    assert!(
        cancellation.is_cancelled(),
        "outcome: {:?}; durable error: {:?}",
        result.outcome,
        result.durable_failure
    );
    assert_eq!(
        result.outcome,
        AttemptOutcome::Completed {
            finish_reason: ModelFinishReason::Stop
        }
    );
    assert!(result.durable_failure.is_none());
    let durable: Vec<_> = store
        .read_events(None, 64)
        .unwrap()
        .events
        .into_iter()
        .map(|envelope| envelope.event)
        .collect();
    let published = observer.events();
    assert_eq!(
        published, durable,
        "every committed event is published once in native order"
    );
    assert!(matches!(
        durable.last(),
        Some(RuntimeEvent::AttemptCompleted { .. })
    ));
    assert_eq!(
        durable
            .iter()
            .filter(|event| matches!(event, RuntimeEvent::AssistantMessageCommitted { .. }))
            .count(),
        1
    );
    assert_eq!(
        durable
            .iter()
            .filter(|event| matches!(event, RuntimeEvent::AttemptCompleted { .. }))
            .count(),
        1
    );
    assert!(
        !durable
            .iter()
            .any(|event| matches!(event, RuntimeEvent::AttemptCancelled { .. }))
    );
    assert_eq!(
        store
            .load_canonical()
            .unwrap()
            .iter()
            .filter(|message| matches!(message, MessageBlock::Assistant(_)))
            .count(),
        1
    );
    assert!(
        store
            .load_unsettled_publication_streams()
            .unwrap()
            .is_empty()
    );
}

/// Failing the final Attempt terminal rolls back the entire accepted answer.
/// A failure terminal/audit may subsequently describe the durable error;
/// neither canonical success nor a successful terminal may be fabricated.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn final_terminal_failure_rolls_back_answer_and_never_publishes_success() {
    let adapter = Arc::new(ScriptedAdapter::new(vec![vec![
        ModelEvent::Started,
        ModelEvent::TextDelta {
            block_index: ContentBlockIndex::new(0),
            text: "done".to_owned(),
        },
        ModelEvent::Completed {
            finish_reason: ModelFinishReason::Stop,
            usage: None,
        },
    ]]));
    let store = Arc::new(
        crate::durable::SqliteConversationStore::in_memory(ConversationId::new(
            "conv_36524fd8-f674-7fc2-8125-06d01fee0e18",
        ))
        .expect("in-memory store"),
    );
    store.arm_fail_next_terminal_event();
    let tool_runtime = tool_runtime_with_store(
        "conv_36524fd8-f674-7fc2-8125-06d01fee0e18",
        Some(store.clone()),
    );
    let (lease, _coordinator) = capability_lease(ToolRegistry::new(), &tool_runtime)
        .await
        .into_lease_and_coordinator();
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let observer = RecordingObserver::default();
    let mut execution = AgentExecution::new(
        request(&adapter),
        lease,
        &cancellation,
        crate::scripted_suites::support::default_execution_policy(),
        runtime(&adapter),
        &tool_runtime,
        crate::agent::AttemptLifecycle::inert(),
    )
    .expect("execution construction");
    execution.observe(&observer);

    let result = execution.run().await;

    assert!(matches!(result.outcome, AttemptOutcome::Failed { .. }));
    assert_eq!(
        result.durable_failure_kind,
        Some(super::DurableFailureKind::CanonicalCommit)
    );
    assert!(result.durable_failure.is_some());
    let events = event_history(store.as_ref());
    assert!(
        events
            .iter()
            .any(|event| matches!(event, RuntimeEvent::AttemptStarted { .. }))
    );
    assert!(
        !events.iter().any(|event| matches!(
            event,
            RuntimeEvent::AttemptCompleted { .. }
                | RuntimeEvent::AssistantMessageCommitted { .. }
                | RuntimeEvent::ModelRequestCompleted { .. }
        )),
        "the rejected final transaction leaves no accepted final facts"
    );
    assert!(
        !observer.events().iter().any(|event| matches!(
            event,
            RuntimeEvent::AttemptCompleted { .. }
                | RuntimeEvent::AssistantMessageCommitted { .. }
                | RuntimeEvent::ModelRequestCompleted { .. }
        )),
        "observers cannot acknowledge rolled-back final facts"
    );
    assert!(
        !store
            .load_canonical()
            .unwrap()
            .iter()
            .any(|message| matches!(message, MessageBlock::Assistant(_)))
    );
    assert_eq!(
        store.terminal_event_attempts(),
        2,
        "one rejected final boundary and one honest failure terminal"
    );
}

#[derive(Default)]
pub(super) struct RecordingObserver {
    pub(super) cancel_after_completion: Option<AgentCancellation>,
    pub(super) cancel_on_admission: Option<(
        AgentCancellation,
        Arc<crate::durable::SqliteConversationStore>,
    )>,
    pub(super) final_publication_probe: Option<Arc<FinalPublicationProbe>>,
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
        journal_sequence: u64,
        _tool_occurrence: Option<&crate::events::types::ToolObservationOccurrence>,
        receipt: Option<&crate::durable::observation::NativeObservationBatch>,
    ) {
        if matches!(event, RuntimeEvent::AttemptStarted { .. })
            && let Some((cancellation, store)) = &self.cancel_on_admission
        {
            let committed = event_history(store.as_ref());
            assert!(matches!(
                committed.as_slice(),
                [
                    RuntimeEvent::AttemptStarted { .. },
                    RuntimeEvent::TurnStarted
                ]
            ));
            let receipt = receipt.expect("first admission receipt");
            assert_eq!(
                receipt.journal_range,
                Some((journal_sequence, journal_sequence + 1))
            );
            cancellation.cancel();
        }
        if let Some(probe) = &self.final_publication_probe {
            match event {
                RuntimeEvent::ModelRequestCompleted { .. } => probe.completion(),
                RuntimeEvent::AttemptCompleted { .. } => probe.terminal(),
                _ => {}
            }
        }
        if matches!(event, RuntimeEvent::ModelRequestCompleted { .. })
            && let Some(cancellation) = &self.cancel_after_completion
        {
            cancellation.cancel();
        }
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
        _receipt: &crate::durable::TranscriptCommitReceipt,
    ) {
        self.committed
            .lock()
            .expect("observer committed lock")
            .push(block.clone());
    }

    fn observe_status(
        &self,
        observation: &crate::agent::AgentStatusObservation,
        _source: Option<&crate::durable::StatusObservationSource>,
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
        _receipt: &crate::durable::PublicationAuditReceipt,
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
            input_schema: serde_json::json!({"type": "object"}).into(),
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

impl FinalPublicationProbe {
    fn completion(self: &Arc<Self>) {
        let probe = Arc::clone(self);
        let (checked, wait_checked) = std::sync::mpsc::sync_channel(0);
        let producer = std::thread::spawn(move || {
            assert!(
                probe
                    .mailbox
                    .with_pending_snapshot(|_| ())
                    .unwrap()
                    .is_none(),
                "the final publication lease excludes a concurrent pending reader"
            );
            checked.send(()).unwrap();
            probe
                .mailbox
                .enqueue(UserMessageBlock {
                    id: MessageId::new("late-after-final"),
                    content: vec![UserContentBlock::Text(crate::message::content::TextBlock {
                        text: "late input".into(),
                    })],
                    source: UserSource::Human,
                    kind: InboundKind::Message,
                    timestamp: Some(chrono::Utc::now()),
                })
                .unwrap();
            assert!(
                probe.terminal.load(std::sync::atomic::Ordering::SeqCst),
                "pending acceptance cannot overtake final terminal publication"
            );
        });
        *self.producer.lock().unwrap() = Some(producer);
        wait_checked.recv().unwrap();
    }

    fn terminal(&self) {
        assert!(
            self.mailbox
                .with_pending_snapshot(|_| ())
                .unwrap()
                .is_none(),
            "the same publication lease remains held through the terminal callback"
        );
        self.terminal
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

include!("../execution_attempt_start_tests.rs");
