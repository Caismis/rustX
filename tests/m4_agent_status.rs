//! M4 Agent Status deterministic regression matrix.
//!
//! Every test is deterministic and network-free except the provider wire
//! translation tests, which drive the real adapters over the local fixture
//! HTTP server (no credentials, no real network). The matrix proves the
//! core invariants of the mandatory Agent Status projection:
//!
//! - structured-before-rendering composition with stable section ids and
//!   deterministic ordering;
//! - explicit fresh-inbound identity (never inferred from role or history
//!   shape);
//! - at most one Agent Status snapshot per request preparation;
//! - the last-in-inbound-order message drives `inbound_message_time`;
//! - fresh inbound is protected from compaction until observed;
//! - Agent Status participates in full token accounting and fingerprinting;
//! - provider adapters own wire placement;
//! - the status never enters canonical history, checkpoints, or results.

mod common;

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use common::context::{FakeContextSummarizer, FakeSummaryStep, ScriptedEstimator};
use common::fake::{FakeModel, FakeStep, model_release};
use rustx::agent::{AgentCancellation, AgentExecution, AgentExecutionRequest};
use rustx::context::{
    AgentStatusAttachment, AgentStatusClock, AgentStatusComposer, AgentStatusCompositionError,
    AgentStatusRenderContext, AgentStatusSectionData, AgentStatusSectionId,
    AgentStatusSectionProvider, ContextCheckpointStore, ContextConfig, ContextEngine, ContextError,
    ContextErrorKind, ContextRuntime, InMemoryCheckpointStore, ProviderObservedInput,
    TokenEstimator, TokenMeasurementSource, render_agent_status,
};
use rustx::events::types::{AttemptOutcome, RuntimeEvent};
use rustx::message::content::TextBlock;
use rustx::message::types::{
    AgentContentBlock, AgentMessageBlock, InboundKind, MessageBlock, ToolMessageBlock,
    UserContentBlock, UserMessageBlock, UserSource,
};
use rustx::model::event::ModelEvent;
use rustx::model::finish::ModelFinishReason;
use rustx::model::types::{ModelProtocol, ModelRequest, ReasoningEffort};
use rustx::runtime::continuation::{OpenAiResponsesContinuation, ProviderContinuationState};
use rustx::runtime::identity::{AgentId, AttemptId, ConversationId, MessageId, ToolCallId, ToolId};
use rustx::runtime::inbound::{ConversationInboundMailbox, FreshInboundTurn};
use rustx::runtime::types::CancellationReason;
use rustx::tools::executor::ToolRegistry;
use rustx::tools::types::{ToolCall, ToolExecutionResult, ToolExecutionStatus};

// ---------------------------------------------------------------------------
// Deterministic clocks and composition fixtures
// ---------------------------------------------------------------------------

/// A clock frozen at one fixed UTC instant.
#[derive(Debug, Clone, Copy)]
struct FixedClock(DateTime<Utc>);

impl AgentStatusClock for FixedClock {
    fn now(&self) -> DateTime<Utc> {
        self.0
    }
}

/// A clock with a scripted sequence of instants: every `now()` pops the next
/// one, so a test can prove that each request preparation composes a fresh
/// status snapshot.
#[derive(Debug)]
struct ScriptedClock {
    times: Mutex<VecDeque<DateTime<Utc>>>,
}

impl ScriptedClock {
    fn new(times: Vec<DateTime<Utc>>) -> Self {
        Self {
            times: Mutex::new(times.into()),
        }
    }
}

impl AgentStatusClock for ScriptedClock {
    fn now(&self) -> DateTime<Utc> {
        self.times
            .lock()
            .expect("scripted clock lock")
            .pop_front()
            .expect("clock script exhausted")
    }
}

/// A fixed deterministic UTC instant.
fn utc(rfc3339: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(rfc3339)
        .expect("fixed timestamp")
        .with_timezone(&Utc)
}

/// A scripted extension status provider: stable id, optional lines, optional
/// failure.
struct TestProvider {
    id: &'static str,
    lines: Option<Vec<String>>,
    fail: bool,
}

impl TestProvider {
    fn returning(id: &'static str, lines: Vec<String>) -> Self {
        Self {
            id,
            lines: Some(lines),
            fail: false,
        }
    }

    fn empty(id: &'static str) -> Self {
        Self {
            id,
            lines: None,
            fail: false,
        }
    }

    fn failing(id: &'static str) -> Self {
        Self {
            id,
            lines: None,
            fail: true,
        }
    }
}

impl AgentStatusSectionProvider for TestProvider {
    fn section_id(&self) -> AgentStatusSectionId {
        AgentStatusSectionId::new(self.id)
    }

    fn section(
        &self,
        _context: &AgentStatusRenderContext,
    ) -> Result<Option<AgentStatusSectionData>, ContextError> {
        if self.fail {
            return Err(ContextError::new(
                ContextErrorKind::StatusFailed,
                "test provider exploded",
            ));
        }
        Ok(self
            .lines
            .clone()
            .map(|lines| AgentStatusSectionData::TextLines { lines }))
    }
}

// ---------------------------------------------------------------------------
// Agent-level fixtures
// ---------------------------------------------------------------------------

fn user(
    id: &str,
    text: &str,
    source: UserSource,
    timestamp: Option<DateTime<Utc>>,
) -> MessageBlock {
    MessageBlock::User(UserMessageBlock {
        id: MessageId::new(id),
        content: vec![UserContentBlock::Text(TextBlock {
            text: text.to_owned(),
        })],
        source,
        kind: InboundKind::Message,
        timestamp,
    })
}

fn historical_user(id: &str, text: &str) -> MessageBlock {
    user(
        id,
        text,
        UserSource::Human,
        Some(utc("2026-08-07T09:00:00Z")),
    )
}

fn fresh_user(id: &str, text: &str, source: UserSource, timestamp: DateTime<Utc>) -> MessageBlock {
    user(id, text, source, Some(timestamp))
}

fn summary_user(id: &str, text: &str) -> MessageBlock {
    MessageBlock::User(UserMessageBlock {
        id: MessageId::new(id),
        content: vec![UserContentBlock::Text(TextBlock {
            text: text.to_owned(),
        })],
        source: UserSource::Runtime,
        kind: InboundKind::CompactionSummary,
        timestamp: None,
    })
}

fn text_block(text: &str) -> AgentContentBlock {
    AgentContentBlock::Text(TextBlock {
        text: text.to_owned(),
    })
}

fn agent(id: &str, blocks: Vec<AgentContentBlock>) -> MessageBlock {
    MessageBlock::Agent(AgentMessageBlock {
        id: MessageId::new(id),
        content: blocks,
    })
}

fn conversation() -> ConversationId {
    ConversationId::new("conv-status-1")
}

fn request(
    attempt: &str,
    initial_messages: Vec<MessageBlock>,
    fresh: Option<FreshInboundTurn>,
    timezone: Option<Tz>,
) -> AgentExecutionRequest {
    AgentExecutionRequest {
        agent_id: AgentId::new("agent-status"),
        conversation_id: conversation(),
        attempt_id: AttemptId::new(attempt),
        initial_messages,
        initial_fresh_inbound: fresh,
        timezone,
        model: "fake-status-model".to_owned(),
        protocol: ModelProtocol::OpenAiChatCompletions,
        reasoning: ReasoningEffort::Medium,
        max_output_tokens: 0,
    }
}

fn engine(
    window: u64,
    reserve: u64,
    keep_recent: u64,
    estimator: Arc<dyn TokenEstimator>,
) -> ContextEngine {
    ContextEngine::new(
        ContextConfig {
            context_window_tokens: window,
            reserve_tokens: reserve,
            keep_recent_tokens: keep_recent,
        },
        estimator,
    )
    .expect("valid context configuration")
}

fn weighted(per_message: u64, per_block: u64, per_tool: u64) -> Arc<ScriptedEstimator> {
    Arc::new(ScriptedEstimator::new(per_message, per_block, per_tool))
}

fn runtime(
    window: u64,
    estimator: Arc<dyn TokenEstimator>,
    summarizer: FakeContextSummarizer,
    store: Arc<InMemoryCheckpointStore>,
    clock: Arc<dyn AgentStatusClock>,
) -> ContextRuntime<'static> {
    ContextRuntime::with_status_composer(
        engine(window, 0, 0, estimator),
        Arc::new(summarizer),
        store,
        AgentStatusComposer::new(clock),
    )
}

fn started() -> ModelEvent {
    ModelEvent::Started
}

fn text_delta(index: u32, delta: &str) -> ModelEvent {
    ModelEvent::TextDelta {
        block_index: rustx::message::types::ContentBlockIndex::new(index),
        text: delta.to_owned(),
    }
}

fn done(reason: ModelFinishReason) -> ModelEvent {
    ModelEvent::Completed {
        finish_reason: reason,
        usage: None,
    }
}

fn overflow() -> ModelEvent {
    ModelEvent::Failed {
        error: rustx::model::ModelError {
            kind: rustx::model::ModelErrorKind::ContextWindowExceeded,
            message: "context window exceeded".to_owned(),
            retry_after_ms: None,
            provider_code: None,
        },
    }
}

fn stop_script() -> Vec<FakeStep> {
    vec![
        FakeStep::Emit(started()),
        FakeStep::Emit(text_delta(0, "ok")),
        FakeStep::Emit(done(ModelFinishReason::Stop)),
    ]
}

fn block_id(message: &MessageBlock) -> String {
    match message {
        MessageBlock::System(system) => system.id.as_str().to_owned(),
        MessageBlock::User(user) => user.id.as_str().to_owned(),
        MessageBlock::Agent(agent) => agent.id.as_str().to_owned(),
        MessageBlock::Tool(tool) => tool.id.as_str().to_owned(),
    }
}

/// Asserts one terminal event, that it is last, and the exact outcome.
fn assert_completed(result: &rustx::agent::AgentExecutionResult) {
    let terminals: Vec<&RuntimeEvent> = result
        .events
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
        .collect();
    assert_eq!(terminals.len(), 1, "exactly one terminal event");
    assert_eq!(
        result.events.last(),
        Some(terminals[0]),
        "no events after the terminal event"
    );
    assert_eq!(
        result.outcome,
        AttemptOutcome::Completed {
            finish_reason: ModelFinishReason::Stop,
        }
    );
}

fn assert_no_status_in_history(history: &[MessageBlock]) {
    let serialized = serde_json::to_string(history).expect("serialize history");
    assert!(
        !serialized.contains("<system-reminder>"),
        "canonical history must never contain the Agent Status footer"
    );
}

// ---------------------------------------------------------------------------
// Structured composition
// ---------------------------------------------------------------------------

#[test]
fn temporal_section_is_first_and_mandatory() {
    let mut composer = AgentStatusComposer::new(Arc::new(FixedClock(utc("2026-08-08T16:31:00Z"))));
    composer
        .register(Arc::new(TestProvider::returning(
            "custom",
            vec!["custom line".to_owned()],
        )))
        .expect("register");
    let status = composer
        .compose(&AgentStatusRenderContext {
            inbound_message_time: utc("2026-08-08T16:30:58Z"),
            timezone: None,
        })
        .expect("compose");
    assert_eq!(status.sections.len(), 2);
    assert_eq!(
        status.sections[0].id.as_str(),
        AgentStatusSectionId::TEMPORAL,
        "the mandatory temporal section is always first"
    );
    assert_eq!(status.sections[1].id.as_str(), "custom");
}

#[test]
fn extensions_preserve_registration_order() {
    let mut composer = AgentStatusComposer::new(Arc::new(FixedClock(utc("2026-08-08T16:31:00Z"))));
    composer
        .register(Arc::new(TestProvider::returning(
            "first",
            vec!["first line".to_owned()],
        )))
        .expect("first");
    composer
        .register(Arc::new(TestProvider::returning(
            "second",
            vec!["second line".to_owned()],
        )))
        .expect("second");
    let status = composer
        .compose(&AgentStatusRenderContext {
            inbound_message_time: utc("2026-08-08T16:30:58Z"),
            timezone: None,
        })
        .expect("compose");
    let ids: Vec<String> = status
        .sections
        .iter()
        .map(|section| section.id.as_str().to_owned())
        .collect();
    assert_eq!(ids, vec!["temporal", "first", "second"]);
    let rendered = render_agent_status(&status);
    assert!(
        rendered.find("first line") < rendered.find("second line"),
        "extension sections render in explicit registration order"
    );
}

#[test]
fn duplicate_extension_id_is_rejected() {
    let mut composer = AgentStatusComposer::new(Arc::new(FixedClock(utc("2026-08-08T16:31:00Z"))));
    composer
        .register(Arc::new(TestProvider::returning("dup", Vec::new())))
        .expect("first registration");
    let error = composer
        .register(Arc::new(TestProvider::returning("dup", Vec::new())))
        .expect_err("duplicate must fail");
    assert!(matches!(
        error,
        AgentStatusCompositionError::DuplicateSectionId(id) if id.as_str() == "dup"
    ));
}

#[test]
fn reserved_temporal_cannot_be_replaced() {
    let mut composer = AgentStatusComposer::new(Arc::new(FixedClock(utc("2026-08-08T16:31:00Z"))));
    let error = composer
        .register(Arc::new(TestProvider::returning("temporal", Vec::new())))
        .expect_err("reserved must fail");
    assert!(matches!(
        error,
        AgentStatusCompositionError::ReservedSectionId(id) if id.as_str() == "temporal"
    ));
}

#[test]
fn background_execution_cannot_be_hijacked() {
    let mut composer = AgentStatusComposer::new(Arc::new(FixedClock(utc("2026-08-08T16:31:00Z"))));
    let error = composer
        .register(Arc::new(TestProvider::returning(
            "background_execution",
            Vec::new(),
        )))
        .expect_err("reserved must fail");
    assert!(matches!(
        error,
        AgentStatusCompositionError::ReservedSectionId(id)
            if id.as_str() == AgentStatusSectionId::BACKGROUND_EXECUTION
    ));
}

#[test]
fn optional_provider_absence_is_deterministic() {
    let mut composer = AgentStatusComposer::new(Arc::new(FixedClock(utc("2026-08-08T16:31:00Z"))));
    composer
        .register(Arc::new(TestProvider::empty("quiet")))
        .expect("register");
    let context = AgentStatusRenderContext {
        inbound_message_time: utc("2026-08-08T16:30:58Z"),
        timezone: None,
    };
    let first = composer.compose(&context).expect("first compose");
    let second = composer.compose(&context).expect("second compose");
    assert_eq!(first, second, "an absent section is deterministic");
    assert_eq!(
        first.sections.len(),
        1,
        "an intentionally absent provider contributes no section"
    );
}

#[test]
fn provider_failure_is_not_silently_omitted() {
    let mut composer = AgentStatusComposer::new(Arc::new(FixedClock(utc("2026-08-08T16:31:00Z"))));
    composer
        .register(Arc::new(TestProvider::failing("broken")))
        .expect("register");
    let error = composer
        .compose(&AgentStatusRenderContext {
            inbound_message_time: utc("2026-08-08T16:30:58Z"),
            timezone: None,
        })
        .expect_err("provider failure must propagate");
    assert_eq!(error.kind, ContextErrorKind::StatusFailed);
    assert!(
        error.message.contains("broken"),
        "the diagnostic names the failing provider"
    );
}

// ---------------------------------------------------------------------------
// Temporal rendering
// ---------------------------------------------------------------------------

#[test]
fn temporal_rendering_is_exact_with_timezone() {
    let composer = AgentStatusComposer::new(Arc::new(FixedClock(utc("2026-08-08T16:31:00Z"))));
    let status = composer
        .compose(&AgentStatusRenderContext {
            inbound_message_time: utc("2026-08-08T16:30:58Z"),
            timezone: Some(Tz::Asia__Tokyo),
        })
        .expect("compose");
    let rendered = render_agent_status(&status);
    assert_eq!(
        rendered,
        "<system-reminder>\n\
         Current time: 2026-08-09T01:31:00+09:00\n\
         Timezone: Asia/Tokyo\n\
         Inbound message time: 2026-08-09T01:30:58+09:00\n\
         </system-reminder>",
        "the exact local rendering must include the RFC3339 numeric offset \
         and the IANA timezone identifier"
    );
}

#[test]
fn temporal_rendering_is_utc_without_timezone_line() {
    let composer = AgentStatusComposer::new(Arc::new(FixedClock(utc("2026-08-08T16:31:00Z"))));
    let status = composer
        .compose(&AgentStatusRenderContext {
            inbound_message_time: utc("2026-08-08T16:30:58Z"),
            timezone: None,
        })
        .expect("compose");
    let rendered = render_agent_status(&status);
    assert_eq!(
        rendered,
        "<system-reminder>\n\
         Current time: 2026-08-08T16:31:00Z\n\
         Inbound message time: 2026-08-08T16:30:58Z\n\
         </system-reminder>",
        "an unknown timezone renders UTC and omits the timezone line"
    );
    assert!(
        !rendered.contains("Timezone"),
        "no timezone line when unknown"
    );
}

// ---------------------------------------------------------------------------
// Initial inbound
// ---------------------------------------------------------------------------

/// A fresh initial human inbound turn produces exactly one Agent Status
/// targeting the inbound message, leaves the canonical message untouched,
/// and never leaks the status into the result history.
#[tokio::test]
async fn initial_human_inbound_produces_exactly_one_status() {
    let model = FakeModel::new(vec![stop_script()]);
    let tools = ToolRegistry::new();
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let store = InMemoryCheckpointStore::new().shared();
    let inbound_time = utc("2026-08-08T16:30:58Z");
    let initial = fresh_user(
        "msg-inbound-1",
        "deploy it",
        UserSource::Human,
        inbound_time,
    );
    let fresh = FreshInboundTurn::new(vec![MessageId::new("msg-inbound-1")]).expect("turn");
    let result = AgentExecution::new(
        request(
            "attempt-1",
            vec![initial.clone()],
            Some(fresh),
            Some(Tz::Asia__Tokyo),
        ),
        &model,
        &tools,
        &cancellation,
        runtime(
            10_000_000,
            weighted(10, 10, 10),
            FakeContextSummarizer::new(Vec::new()),
            store,
            Arc::new(FixedClock(utc("2026-08-08T16:31:00Z"))),
        ),
    )
    .run()
    .await;
    assert_completed(&result);

    let requests = model.requests();
    assert_eq!(requests.len(), 1, "one model turn");
    let status = requests[0]
        .agent_status
        .as_ref()
        .expect("exactly one Agent Status snapshot");
    assert_eq!(status.target_message_id, MessageId::new("msg-inbound-1"));
    assert_eq!(
        status.rendered,
        "<system-reminder>\n\
         Current time: 2026-08-09T01:31:00+09:00\n\
         Timezone: Asia/Tokyo\n\
         Inbound message time: 2026-08-09T01:30:58+09:00\n\
         </system-reminder>"
    );
    // The canonical user message is untouched: no status text in its content.
    assert_eq!(requests[0].messages.len(), 1);
    assert_eq!(&requests[0].messages[0], &initial);
    // The result history carries the canonical messages only (the inbound
    // user message and the committed agent turn), never the projection-only
    // status artifact.
    assert_eq!(result.messages.len(), 2);
    assert_no_status_in_history(&result.messages);
}

/// A runtime-originated fresh inbound turn triggers Agent Status exactly
/// like a human turn: status triggering is provenance-neutral.
#[tokio::test]
async fn runtime_originated_inbound_triggers_status() {
    let model = FakeModel::new(vec![stop_script()]);
    let tools = ToolRegistry::new();
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let store = InMemoryCheckpointStore::new().shared();
    let inbound_time = utc("2026-08-08T16:30:58Z");
    let initial = fresh_user(
        "msg-runtime-1",
        "background task finished",
        UserSource::Runtime,
        inbound_time,
    );
    let fresh = FreshInboundTurn::new(vec![MessageId::new("msg-runtime-1")]).expect("turn");
    let result = AgentExecution::new(
        request("attempt-1", vec![initial], Some(fresh), None),
        &model,
        &tools,
        &cancellation,
        runtime(
            10_000_000,
            weighted(10, 10, 10),
            FakeContextSummarizer::new(Vec::new()),
            store,
            Arc::new(FixedClock(utc("2026-08-08T16:31:00Z"))),
        ),
    )
    .run()
    .await;
    assert_completed(&result);

    let requests = model.requests();
    assert_eq!(requests.len(), 1);
    let status = requests[0].agent_status.as_ref().expect("status");
    assert_eq!(status.target_message_id, MessageId::new("msg-runtime-1"));
    assert!(
        status
            .rendered
            .contains("Inbound message time: 2026-08-08T16:30:58Z"),
        "the runtime inbound message timestamp drives inbound_message_time"
    );
    assert_no_status_in_history(&result.messages);
}

/// Freshness is never inferred from role or history shape: a historical
/// compaction summary (Runtime source, no timestamp, not marked fresh) and an
/// unmarked human message never produce Agent Status.
#[tokio::test]
async fn no_role_heuristic_triggers_status() {
    let model = FakeModel::new(vec![stop_script()]);
    let tools = ToolRegistry::new();
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let store = InMemoryCheckpointStore::new().shared();
    let initial = vec![
        summary_user("msg-summary-1", "earlier history"),
        historical_user("msg-old-1", "old message"),
    ];
    let result = AgentExecution::new(
        request("attempt-1", initial, None, None),
        &model,
        &tools,
        &cancellation,
        runtime(
            10_000_000,
            weighted(10, 10, 10),
            FakeContextSummarizer::new(Vec::new()),
            store,
            Arc::new(FixedClock(utc("2026-08-08T16:31:00Z"))),
        ),
    )
    .run()
    .await;
    assert_completed(&result);
    let requests = model.requests();
    assert_eq!(requests.len(), 1);
    assert!(
        requests[0].agent_status.is_none(),
        "user-role history without an explicit fresh trigger must never carry Agent Status"
    );
}

// ---------------------------------------------------------------------------
// Drained batches
// ---------------------------------------------------------------------------

/// A drained A/B batch appends two distinct canonical messages and produces
/// exactly one status snapshot targeting B, with `inbound_message_time`
/// equal to B's persisted timestamp. Mixed Human/Runtime provenance works.
#[tokio::test]
async fn drained_batch_produces_one_status_targeting_the_final_message() {
    let model = FakeModel::new(vec![stop_script(), stop_script()]);
    let tools = ToolRegistry::new();
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let store = InMemoryCheckpointStore::new().shared();
    let mailbox = ConversationInboundMailbox::new(conversation());
    mailbox
        .enqueue(UserMessageBlock {
            id: MessageId::new("msg-a"),
            content: vec![UserContentBlock::Text(TextBlock {
                text: "A".to_owned(),
            })],
            source: UserSource::Human,
            kind: InboundKind::Message,
            timestamp: Some(utc("2026-08-07T12:00:00Z")),
        })
        .expect("enqueue A");
    mailbox
        .enqueue(UserMessageBlock {
            id: MessageId::new("msg-b"),
            content: vec![UserContentBlock::Text(TextBlock {
                text: "B".to_owned(),
            })],
            source: UserSource::Runtime,
            kind: InboundKind::Message,
            timestamp: Some(utc("2026-08-07T12:30:00Z")),
        })
        .expect("enqueue B");
    let result = AgentExecution::new(
        request(
            "attempt-1",
            vec![historical_user("msg-u0", "start")],
            None,
            None,
        ),
        &model,
        &tools,
        &cancellation,
        runtime(
            10_000_000,
            weighted(10, 10, 10),
            FakeContextSummarizer::new(Vec::new()),
            store,
            Arc::new(FixedClock(utc("2026-08-07T13:00:00Z"))),
        ),
    )
    .with_inbound_mailbox(mailbox)
    .expect("mailbox bound")
    .run()
    .await;
    assert_completed(&result);

    let requests = model.requests();
    assert_eq!(
        requests.len(),
        2,
        "initial turn plus the drained-batch turn"
    );
    assert!(
        requests[0].agent_status.is_none(),
        "no fresh inbound before the drain"
    );
    // A and B remain distinct canonical messages in the next request.
    let ids: Vec<String> = requests[1].messages.iter().map(block_id).collect();
    assert_eq!(
        ids,
        vec![
            "msg-u0".to_owned(),
            "attempt-1-agent-1".to_owned(),
            "msg-a".to_owned(),
            "msg-b".to_owned(),
        ],
        "A and B are distinct canonical messages in inbound order"
    );
    let status = requests[1]
        .agent_status
        .as_ref()
        .expect("exactly one status snapshot for the batch");
    assert_eq!(status.target_message_id, MessageId::new("msg-b"));
    assert!(
        status
            .rendered
            .contains("Inbound message time: 2026-08-07T12:30:00Z"),
        "inbound_message_time is B's persisted timestamp"
    );
    // The status appears exactly once across the whole batch request.
    let all_serialized = serde_json::to_string(&requests[1]).expect("serialize");
    assert_eq!(
        all_serialized.matches("<system-reminder>").count(),
        1,
        "at most one status snapshot per request"
    );
    assert_no_status_in_history(&result.messages);
}

/// The mailbox sequence is the delivery-order authority: producer wall-clock
/// timestamps may be non-monotonic. A has a later timestamp but an earlier
/// sequence; `inbound_message_time` must be B's (earlier) timestamp because
/// B is the final message in inbound order — never `max(timestamp)`.
#[tokio::test]
async fn non_monotonic_producer_timestamps_follow_inbound_order() {
    let model = FakeModel::new(vec![stop_script(), stop_script()]);
    let tools = ToolRegistry::new();
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let store = InMemoryCheckpointStore::new().shared();
    let mailbox = ConversationInboundMailbox::new(conversation());
    // A: sequence 1, LATER wall-clock timestamp.
    mailbox
        .enqueue(UserMessageBlock {
            id: MessageId::new("msg-a"),
            content: vec![UserContentBlock::Text(TextBlock {
                text: "A".to_owned(),
            })],
            source: UserSource::Human,
            kind: InboundKind::Message,
            timestamp: Some(utc("2026-08-08T12:00:00Z")),
        })
        .expect("enqueue A");
    // B: sequence 2, EARLIER wall-clock timestamp.
    mailbox
        .enqueue(UserMessageBlock {
            id: MessageId::new("msg-b"),
            content: vec![UserContentBlock::Text(TextBlock {
                text: "B".to_owned(),
            })],
            source: UserSource::Human,
            kind: InboundKind::Message,
            timestamp: Some(utc("2026-08-08T11:00:00Z")),
        })
        .expect("enqueue B");
    let result = AgentExecution::new(
        request(
            "attempt-1",
            vec![historical_user("msg-u0", "start")],
            None,
            None,
        ),
        &model,
        &tools,
        &cancellation,
        runtime(
            10_000_000,
            weighted(10, 10, 10),
            FakeContextSummarizer::new(Vec::new()),
            store,
            Arc::new(FixedClock(utc("2026-08-08T13:00:00Z"))),
        ),
    )
    .with_inbound_mailbox(mailbox)
    .expect("mailbox bound")
    .run()
    .await;
    assert_completed(&result);

    let requests = model.requests();
    assert_eq!(requests.len(), 2);
    let status = requests[1].agent_status.as_ref().expect("status");
    assert_eq!(status.target_message_id, MessageId::new("msg-b"));
    let rendered = &status.rendered;
    assert!(
        rendered.contains("Inbound message time: 2026-08-08T11:00:00Z"),
        "the final message in inbound order drives inbound_message_time: {rendered}"
    );
    assert!(
        !rendered.contains("Inbound message time: 2026-08-08T12:00:00Z"),
        "max(timestamp) must never drive inbound_message_time"
    );
}

/// A correction batch ("deploy it" then "actually do not deploy it") is
/// drained atomically: the next provider request observes both messages in
/// order with exactly one status after the final one, and no intermediate
/// request containing only A ever exists.
#[tokio::test]
async fn correction_batch_reaches_the_model_as_one_turn() {
    let model = FakeModel::new(vec![stop_script(), stop_script()]);
    let tools = ToolRegistry::new();
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let store = InMemoryCheckpointStore::new().shared();
    let mailbox = ConversationInboundMailbox::new(conversation());
    mailbox
        .enqueue(UserMessageBlock {
            id: MessageId::new("msg-a"),
            content: vec![UserContentBlock::Text(TextBlock {
                text: "deploy it".to_owned(),
            })],
            source: UserSource::Human,
            kind: InboundKind::Message,
            timestamp: Some(utc("2026-08-07T12:00:00Z")),
        })
        .expect("enqueue A");
    mailbox
        .enqueue(UserMessageBlock {
            id: MessageId::new("msg-b"),
            content: vec![UserContentBlock::Text(TextBlock {
                text: "actually do not deploy it".to_owned(),
            })],
            source: UserSource::Human,
            kind: InboundKind::Message,
            timestamp: Some(utc("2026-08-07T12:01:00Z")),
        })
        .expect("enqueue B");
    let result = AgentExecution::new(
        request(
            "attempt-1",
            vec![historical_user("msg-u0", "start")],
            None,
            None,
        ),
        &model,
        &tools,
        &cancellation,
        runtime(
            10_000_000,
            weighted(10, 10, 10),
            FakeContextSummarizer::new(Vec::new()),
            store,
            Arc::new(FixedClock(utc("2026-08-07T13:00:00Z"))),
        ),
    )
    .with_inbound_mailbox(mailbox)
    .expect("mailbox bound")
    .run()
    .await;
    assert_completed(&result);

    let requests = model.requests();
    assert_eq!(
        requests.len(),
        2,
        "exactly two model requests: no intermediate request may observe only A"
    );
    let batch_request = &requests[1];
    let texts: Vec<String> = batch_request
        .messages
        .iter()
        .filter_map(|message| match message {
            MessageBlock::User(user) => Some(
                user.content
                    .iter()
                    .filter_map(|block| match block {
                        UserContentBlock::Text(text) => Some(text.text.clone()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join(" "),
            ),
            _ => None,
        })
        .collect();
    assert!(
        texts
            .windows(2)
            .any(|pair| pair[0].contains("deploy it")
                && pair[1].contains("actually do not deploy it")),
        "the batch reaches the model as ordered A then B: {texts:?}"
    );
    let status = batch_request.agent_status.as_ref().expect("one status");
    assert_eq!(status.target_message_id, MessageId::new("msg-b"));
    assert_no_status_in_history(&result.messages);
}

/// A foreground-tool-only continuation never receives Agent Status: the
/// fresh inbound turn was consumed by the successful tool-calling turn, and
/// no mailbox batch was drained afterward.
#[tokio::test]
async fn foreground_tool_continuation_has_no_status() {
    use common::fake::ScriptedCall;
    use common::fake::tool_call_events;
    let call = ScriptedCall {
        id: "call-1",
        tool_id: "tool-alpha",
        name: "alpha",
        arguments: serde_json::json!({}),
    };
    let mut tool_script: Vec<FakeStep> = std::iter::once(FakeStep::Emit(started()))
        .chain(tool_call_events(0, &call).into_iter().map(FakeStep::Emit))
        .collect();
    tool_script.push(FakeStep::Emit(done(ModelFinishReason::ToolCalls)));
    let model = FakeModel::new(vec![tool_script, stop_script()]);
    let mut tools = ToolRegistry::new();
    tools.insert(common::fake::FakeTool::new(
        common::tool("alpha", "tool-alpha"),
        common::fake::success_result("ok"),
    ));
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let store = InMemoryCheckpointStore::new().shared();
    let inbound_time = utc("2026-08-07T12:00:00Z");
    let initial = fresh_user("msg-inbound-1", "run it", UserSource::Human, inbound_time);
    let fresh = FreshInboundTurn::new(vec![MessageId::new("msg-inbound-1")]).expect("turn");
    let result = AgentExecution::new(
        request("attempt-1", vec![initial], Some(fresh), None),
        &model,
        &tools,
        &cancellation,
        runtime(
            10_000_000,
            weighted(10, 10, 10),
            FakeContextSummarizer::new(Vec::new()),
            store,
            Arc::new(FixedClock(utc("2026-08-07T13:00:00Z"))),
        ),
    )
    .run()
    .await;
    assert_completed(&result);

    let requests = model.requests();
    assert_eq!(requests.len(), 2, "tool turn plus its continuation");
    assert!(
        requests[0].agent_status.is_some(),
        "the fresh inbound turn carries one status"
    );
    assert!(
        requests[1].agent_status.is_none(),
        "the foreground-tool-only continuation must carry no Agent Status"
    );
    assert_no_status_in_history(&result.messages);
}

// ---------------------------------------------------------------------------
// Compaction protection and accounting
// ---------------------------------------------------------------------------

/// A fresh inbound turn that has not yet been observed is protected from
/// compaction: older history compacts, the fresh message stays literal, and
/// the status snapshot still targets it.
#[tokio::test]
async fn fresh_inbound_is_protected_from_compaction() {
    let model = FakeModel::new(vec![stop_script()]);
    let tools = ToolRegistry::new();
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let store = InMemoryCheckpointStore::new().shared();
    let summarizer = FakeContextSummarizer::new(vec![FakeSummaryStep::Return("S".to_owned())]);
    let inbound_time = utc("2026-08-07T12:00:00Z");
    let initial = vec![
        historical_user("msg-old-1", "old"),
        historical_user("msg-old-2", "older"),
        fresh_user(
            "msg-inbound-1",
            "fresh instruction",
            UserSource::Human,
            inbound_time,
        ),
    ];
    let fresh = FreshInboundTurn::new(vec![MessageId::new("msg-inbound-1")]).expect("turn");
    let result = AgentExecution::new(
        request("attempt-1", initial, Some(fresh), None),
        &model,
        &tools,
        &cancellation,
        runtime(
            250,
            weighted(100, 10, 0),
            summarizer,
            store.clone(),
            Arc::new(FixedClock(utc("2026-08-07T13:00:00Z"))),
        ),
    )
    .run()
    .await;
    assert_completed(&result);

    // Exactly one proactive compaction ran: 300 tokens of history with the
    // status snapshot cross the soft limit of 250.
    let compactions = result
        .events
        .iter()
        .filter(|event| matches!(event, RuntimeEvent::CompactionStarted))
        .count();
    assert_eq!(
        compactions, 1,
        "the fresh projection must cross the threshold"
    );
    let requests = model.requests();
    assert_eq!(requests.len(), 1);
    let ids: Vec<String> = requests[0].messages.iter().map(block_id).collect();
    assert_eq!(
        ids,
        vec![
            rustx::context::summary_message_id(&conversation(), 1).to_string(),
            "msg-inbound-1".to_owned(),
        ],
        "older history compacts away but the fresh inbound message stays literal"
    );
    let status = requests[0].agent_status.as_ref().expect("status");
    assert_eq!(status.target_message_id, MessageId::new("msg-inbound-1"));
    // Canonical history is untouched by compaction and status.
    let history_ids: Vec<String> = result.messages.iter().map(block_id).collect();
    assert_eq!(
        history_ids,
        vec![
            "msg-old-1".to_owned(),
            "msg-old-2".to_owned(),
            "msg-inbound-1".to_owned(),
            "attempt-1-agent-1".to_owned(),
        ]
    );
    assert_no_status_in_history(&result.messages);
    // The checkpoint itself never contains the status footer.
    let checkpoint = store
        .load(&conversation())
        .expect("store")
        .expect("checkpoint exists");
    assert!(
        !serde_json::to_string(&checkpoint)
            .expect("serialize checkpoint")
            .contains("<system-reminder>"),
        "the checkpoint must never contain the Agent Status footer"
    );
}

/// When preserving the fresh inbound material makes the projection
/// impossible, planning fails with `CannotFit` instead of summarizing the
/// unobserved instruction. Here the only compactable message is the fresh
/// inbound message itself.
#[test]
fn unobservable_fresh_inbound_yields_cannot_fit_not_summary() {
    let engine = engine(10_000_000, 0, 0, weighted(10, 10, 10));
    let history = vec![fresh_user(
        "msg-inbound-1",
        "do not summarize me",
        UserSource::Human,
        utc("2026-08-07T12:00:00Z"),
    )];
    let projection = engine
        .build_projection(&history, None, &[], None, None)
        .expect("projection");
    let fresh = FreshInboundTurn::new(vec![MessageId::new("msg-inbound-1")]).expect("turn");
    let error = engine
        .plan_compaction(
            &history,
            None,
            &projection,
            &[],
            0,
            &rustx::context::CompactionConstraints {
                must_cover_through: None,
                fresh_inbound: Some(&fresh),
            },
        )
        .expect_err("the only cut would retire the fresh inbound message");
    assert_eq!(
        error.kind,
        ContextErrorKind::CannotFit,
        "an impossible fresh-inbound-preserving projection must fail explicitly"
    );
}

/// The exact Agent Status snapshot contributes to the full request estimate:
/// history without status sits below the soft limit, and the same history
/// with status reaches the limit — equality triggers compaction.
#[test]
fn status_snapshot_changes_the_compaction_decision() {
    let engine = engine(100, 0, 5, weighted(10, 10, 10));
    let history = vec![user(
        "msg-u1",
        "hi",
        UserSource::Human,
        Some(utc("2026-08-07T12:00:00Z")),
    )];
    let without_status = engine
        .build_projection(&history, None, &[], None, None)
        .expect("projection");
    // Soft limit with max_output_tokens = 10: 100 - 0 - 10 = 90.
    assert!(
        !engine
            .should_compact(&without_status, 10)
            .expect("threshold decision"),
        "history without status is below the soft limit"
    );
    // Status weight is ceil(bytes / 4): a 318-byte footer contributes 80
    // tokens, so history + status == 90 == soft limit.
    let with_status = engine
        .build_projection(
            &history,
            None,
            &[],
            None,
            Some(&AgentStatusAttachment {
                target_message_id: MessageId::new("msg-u1"),
                rendered: "x".repeat(318),
            }),
        )
        .expect("projection with status");
    assert_eq!(
        with_status.estimated_input.input_tokens, 90,
        "history + status must equal the soft limit"
    );
    assert!(
        engine
            .should_compact(&with_status, 10)
            .expect("threshold decision"),
        "equality at the soft limit must trigger compaction"
    );
}

/// A different status snapshot changes the projection fingerprint, so a
/// provider-reported measurement of the old projection is never reused for
/// the new one.
#[test]
fn status_snapshot_changes_fingerprint_and_observed_measurement_scope() {
    let engine = engine(10_000_000, 0, 0, weighted(10, 10, 10));
    let history = vec![user(
        "msg-u1",
        "hi",
        UserSource::Human,
        Some(utc("2026-08-07T12:00:00Z")),
    )];
    let snapshot_one = AgentStatusAttachment {
        target_message_id: MessageId::new("msg-u1"),
        rendered: "<system-reminder>\nCurrent time: 2026-08-08T16:31:00Z\n</system-reminder>"
            .to_owned(),
    };
    let snapshot_two = AgentStatusAttachment {
        target_message_id: MessageId::new("msg-u1"),
        rendered: "<system-reminder>\nCurrent time: 2026-08-08T16:32:00Z\n</system-reminder>"
            .to_owned(),
    };
    let observed = ProviderObservedInput {
        fingerprint: engine
            .build_projection(&history, None, &[], None, Some(&snapshot_one))
            .expect("projection one")
            .fingerprint(),
        input_tokens: 42,
    };
    let with_one = engine
        .build_projection(&history, None, &[], Some(&observed), Some(&snapshot_one))
        .expect("projection one measured");
    assert_eq!(
        with_one.estimated_input.source,
        TokenMeasurementSource::ProviderReported
    );
    let with_two = engine
        .build_projection(&history, None, &[], Some(&observed), Some(&snapshot_two))
        .expect("projection two");
    assert_eq!(
        with_two.estimated_input.source,
        TokenMeasurementSource::Estimated,
        "a different status snapshot must invalidate the old observed measurement"
    );
}

/// A fresh inbound batch surviving compaction keeps exactly one status, and
/// the retry after a context overflow composes a freshly sampled snapshot
/// while retaining the same fresh inbound turn.
#[tokio::test]
#[allow(clippy::too_many_lines)] // the full overflow/retry/status lifecycle is asserted verbatim
async fn overflow_retry_composes_a_fresh_status_snapshot() {
    use rustx::runtime::continuation::{AnthropicContinuation, ProviderContinuationState};
    let anthropic_state = ProviderContinuationState::Anthropic(AnthropicContinuation {
        opaque: serde_json::json!({"type": "thinking", "thinking": "x", "signature": "sig"}),
    });
    let (release, parked) = model_release();
    let model = FakeModel::new(vec![
        // Turn 1: a successful Stop with a continuation; the controller
        // enqueues the A/B batch while it streams.
        vec![
            FakeStep::Emit(started()),
            FakeStep::Emit(ModelEvent::ContinuationState {
                block_index: rustx::message::types::ContentBlockIndex::new(0),
                state: anthropic_state.clone(),
            }),
            FakeStep::Emit(text_delta(1, "done")),
            FakeStep::ParkUntilReleased(parked.clone()),
            FakeStep::Emit(done(ModelFinishReason::Stop)),
        ],
        // Turn 2: the drained batch overflows the provider.
        vec![FakeStep::Emit(started()), FakeStep::Emit(overflow())],
        // Turn 3: the compacted retry succeeds.
        vec![
            FakeStep::Emit(started()),
            FakeStep::Emit(text_delta(0, "retried")),
            FakeStep::Emit(done(ModelFinishReason::Stop)),
        ],
    ]);
    let tools = ToolRegistry::new();
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let store = InMemoryCheckpointStore::new().shared();
    let summarizer = FakeContextSummarizer::new(vec![FakeSummaryStep::Return("S1".to_owned())]);
    let mailbox = ConversationInboundMailbox::new(conversation());
    let controller_mailbox = mailbox.clone();
    let mut model_parked = model.parked();
    let controller = tokio::spawn(async move {
        model_parked
            .wait_for(|is_parked| *is_parked)
            .await
            .expect("model parked");
        controller_mailbox
            .enqueue(UserMessageBlock {
                id: MessageId::new("msg-a"),
                content: vec![UserContentBlock::Text(TextBlock {
                    text: "A".to_owned(),
                })],
                source: UserSource::Human,
                kind: InboundKind::Message,
                timestamp: Some(utc("2026-08-07T12:00:00Z")),
            })
            .expect("enqueue A");
        controller_mailbox
            .enqueue(UserMessageBlock {
                id: MessageId::new("msg-b"),
                content: vec![UserContentBlock::Text(TextBlock {
                    text: "B".to_owned(),
                })],
                source: UserSource::Human,
                kind: InboundKind::Message,
                timestamp: Some(utc("2026-08-07T12:01:00Z")),
            })
            .expect("enqueue B");
        release.send_replace(true);
    });
    // Window 400: the turn-2 projection (u0 + agent-1 + A + B + status =
    // 338) fits, but the provider overflows anyway; the retry compacts.
    let result = AgentExecution::new(
        request(
            "attempt-1",
            vec![historical_user("msg-u0", "start")],
            None,
            None,
        ),
        &model,
        &tools,
        &cancellation,
        runtime(
            400,
            weighted(100, 10, 0),
            summarizer,
            store.clone(),
            Arc::new(ScriptedClock::new(vec![
                utc("2026-08-07T13:00:00Z"),
                utc("2026-08-07T13:05:00Z"),
            ])),
        ),
    )
    .with_inbound_mailbox(mailbox)
    .expect("mailbox bound")
    .run()
    .await;
    controller.await.expect("controller task");
    assert_completed(&result);

    let requests = model.requests();
    assert_eq!(
        requests.len(),
        3,
        "initial turn, overflowing turn, retry turn"
    );
    // The overflow request carried status snapshot S1; the retry carries a
    // freshly composed snapshot S2 from a different clock instant, still
    // targeting the same fresh inbound message.
    let first_status = requests[1].agent_status.as_ref().expect("S1");
    let retry_status = requests[2].agent_status.as_ref().expect("S2");
    assert_eq!(first_status.target_message_id, MessageId::new("msg-b"));
    assert_eq!(retry_status.target_message_id, MessageId::new("msg-b"));
    assert!(
        first_status
            .rendered
            .contains("Current time: 2026-08-07T13:00:00Z"),
        "S1 samples the first clock instant"
    );
    assert!(
        retry_status
            .rendered
            .contains("Current time: 2026-08-07T13:05:00Z"),
        "the retry is a new request preparation with a freshly sampled S2"
    );
    for request in &requests {
        let serialized = serde_json::to_string(request).expect("serialize");
        assert_eq!(
            serialized.matches("<system-reminder>").count(),
            usize::from(request.agent_status.is_some()),
            "exactly one status footer per request carrying status"
        );
    }
    // The retry request continues on the compacted projection: the fresh
    // A/B messages stay literal, the summary stands for the older history,
    // and the successful compaction invalidated the pending continuation.
    let retry_ids: Vec<String> = requests[2].messages.iter().map(block_id).collect();
    assert_eq!(
        retry_ids,
        vec![
            rustx::context::summary_message_id(&conversation(), 1).to_string(),
            "msg-a".to_owned(),
            "msg-b".to_owned(),
        ]
    );
    assert!(
        requests[2].continuation.is_none(),
        "successful compaction remains the only continuation invalidation boundary"
    );
    // Canonical history is unchanged by status composition: no footer, no
    // fabricated message.
    let history_ids: Vec<String> = result.messages.iter().map(block_id).collect();
    assert_eq!(
        history_ids,
        vec![
            "msg-u0".to_owned(),
            "attempt-1-agent-1".to_owned(),
            "msg-a".to_owned(),
            "msg-b".to_owned(),
            "attempt-1-agent-2-retry-1".to_owned(),
        ]
    );
    assert_no_status_in_history(&result.messages);
    let checkpoint = store
        .load(&conversation())
        .expect("store")
        .expect("checkpoint");
    assert!(
        !serde_json::to_string(&checkpoint)
            .expect("serialize checkpoint")
            .contains("<system-reminder>"),
        "the checkpoint must never contain the Agent Status footer"
    );
}

// ---------------------------------------------------------------------------
// Provider wire translation
// ---------------------------------------------------------------------------

const STATUS_TEXT: &str = "<system-reminder>\nCurrent time: 2026-08-07T13:00:00Z\nInbound message time: 2026-08-07T12:01:00Z\n</system-reminder>";

fn status_request(protocol: ModelProtocol, messages: Vec<MessageBlock>) -> ModelRequest {
    ModelRequest {
        model: "status-test".to_owned(),
        protocol,
        messages,
        tools: Vec::new(),
        agent_status: Some(AgentStatusAttachment {
            target_message_id: MessageId::new("msg-b"),
            rendered: STATUS_TEXT.to_owned(),
        }),
        reasoning: ReasoningEffort::Medium,
        max_output_tokens: 64,
        continuation: None,
    }
}

fn inbound_user(id: &str, text: &str) -> MessageBlock {
    user(
        id,
        text,
        UserSource::Human,
        Some(utc("2026-08-07T12:00:00Z")),
    )
}

/// Counts decoded string values equal to `needle` across a parsed wire
/// document. The wire body JSON-escapes newlines, so raw body-string matching
/// cannot be used.
fn count_text_values(value: &serde_json::Value, needle: &str) -> usize {
    match value {
        serde_json::Value::String(text) => usize::from(text == needle),
        serde_json::Value::Array(items) => items
            .iter()
            .map(|item| count_text_values(item, needle))
            .sum(),
        serde_json::Value::Object(map) => map
            .values()
            .map(|item| count_text_values(item, needle))
            .sum(),
        _ => 0,
    }
}

#[tokio::test]
async fn chat_completions_appends_status_to_the_target_fresh_user() {
    let server = common::FixtureServer::start(|_attempt, _head| {
        common::sse_fixture("openai_chat", "plain_text.sse")
    })
    .await;
    let adapter = rustx::model::OpenAiChatCompletionsAdapter::new(
        rustx::model::OpenAiAdapterConfig::new("test-key").with_api_base(server.url("/v1")),
    );
    let request = status_request(
        ModelProtocol::OpenAiChatCompletions,
        vec![
            inbound_user("msg-a", "deploy it"),
            inbound_user("msg-b", "actually do not deploy it"),
        ],
    );
    let events = common::collect_events(&adapter, request).await;
    assert!(matches!(events.last(), Some(ModelEvent::Completed { .. })));
    let body: serde_json::Value =
        serde_json::from_str(&server.request_body(0)).expect("request body is JSON");
    let messages = body["messages"].as_array().expect("messages array");
    assert_eq!(messages.len(), 2, "A and B remain distinct wire messages");
    let texts: Vec<String> = messages
        .iter()
        .map(|message| {
            message["content"]
                .as_array()
                .map(|parts| {
                    parts
                        .iter()
                        .filter_map(|part| part["text"].as_str().map(str::to_owned))
                        .collect::<Vec<_>>()
                        .join(" | ")
                })
                .unwrap_or_default()
        })
        .collect();
    assert_eq!(
        texts[0], "deploy it",
        "A is its own message with no status footer"
    );
    assert!(
        texts[1].starts_with("actually do not deploy it | "),
        "B's existing content stays first: {}",
        texts[1]
    );
    assert!(
        texts[1].ends_with(STATUS_TEXT),
        "the rendered status is the final text part of B: {}",
        texts[1]
    );
    assert_eq!(
        count_text_values(&body, STATUS_TEXT),
        1,
        "the status appears exactly once on the wire"
    );
}

#[tokio::test]
async fn chat_completions_continuation_without_status_has_no_footer() {
    let server = common::FixtureServer::start(|_attempt, _head| {
        common::sse_fixture("openai_chat", "plain_text.sse")
    })
    .await;
    let adapter = rustx::model::OpenAiChatCompletionsAdapter::new(
        rustx::model::OpenAiAdapterConfig::new("test-key").with_api_base(server.url("/v1")),
    );
    let call = common::fake::ScriptedCall {
        id: "call-1",
        tool_id: "tool-alpha",
        name: "alpha",
        arguments: serde_json::json!({}),
    };
    let mut request = status_request(
        ModelProtocol::OpenAiChatCompletions,
        vec![
            inbound_user("msg-u0", "start"),
            agent(
                "attempt-1-agent-1",
                vec![
                    text_block("calling"),
                    AgentContentBlock::ToolCall(ToolCall {
                        id: ToolCallId::new("call-1"),
                        tool_id: ToolId::new("tool-alpha"),
                        name: "alpha".to_owned(),
                        arguments: serde_json::json!({}),
                    }),
                ],
            ),
            MessageBlock::Tool(ToolMessageBlock {
                id: MessageId::new("msg-tool-1"),
                tool_call_id: ToolCallId::new("call-1"),
                tool_id: ToolId::new("tool-alpha"),
                result: ToolExecutionResult {
                    status: ToolExecutionStatus::Success,
                    content: vec![rustx::tools::types::ToolResultContent::Text(TextBlock {
                        text: "ok".to_owned(),
                    })],
                    duration_ms: 1,
                    exit_code: Some(0),
                    artifacts: Vec::new(),
                    truncation: None,
                },
            }),
        ],
    );
    request.agent_status = None;
    let _events = common::collect_events(&adapter, request).await;
    let body = server.request_body(0);
    assert!(
        !body.contains("<system-reminder>"),
        "a foreground-tool continuation without a fresh inbound turn carries no status footer"
    );
    let _ = call;
}

#[tokio::test]
async fn responses_stored_continuation_appends_status_in_the_transmitted_tail() {
    let server = common::FixtureServer::start(|_attempt, _head| {
        common::sse_fixture("openai_responses", "stored_completed.sse")
    })
    .await;
    let adapter = rustx::model::OpenAiResponsesAdapter::new(
        rustx::model::OpenAiAdapterConfig::new("test-key").with_api_base(server.url("/v1")),
    );
    let mut request = status_request(
        ModelProtocol::OpenAiResponses,
        vec![
            agent("attempt-1-agent-1", vec![text_block("previous turn")]),
            inbound_user("msg-a", "deploy it"),
            inbound_user("msg-b", "actually do not deploy it"),
        ],
    );
    request.continuation = Some(ProviderContinuationState::OpenAiResponses(
        OpenAiResponsesContinuation::Stored {
            previous_response_id: "resp_prev".to_owned(),
        },
    ));
    let events = common::collect_events(&adapter, request).await;
    assert!(matches!(events.last(), Some(ModelEvent::Completed { .. })));
    let body: serde_json::Value =
        serde_json::from_str(&server.request_body(0)).expect("request body is JSON");
    assert_eq!(body["previous_response_id"], "resp_prev");
    let input = body["input"].as_array().expect("input items");
    // The stored continuation slices the canonical request: only the agent
    // boundary and the tail after it are transmitted.
    let user_texts: Vec<String> = input
        .iter()
        .filter_map(|item| {
            if item["type"] != "message" || item["role"] != "user" {
                return None;
            }
            Some(
                item["content"]
                    .as_array()
                    .map(|parts| {
                        parts
                            .iter()
                            .filter_map(|part| part["text"].as_str().map(str::to_owned))
                            .collect::<Vec<_>>()
                            .join(" | ")
                    })
                    .unwrap_or_default(),
            )
        })
        .collect();
    assert_eq!(
        user_texts,
        vec![
            "deploy it".to_owned(),
            format!("actually do not deploy it | {STATUS_TEXT}")
        ],
        "the tail keeps A and B ordered, with the status as the final input_text unit of B"
    );
    assert_eq!(
        count_text_values(&body, STATUS_TEXT),
        1,
        "the status appears exactly once in the transmitted tail"
    );
}

#[tokio::test]
async fn responses_stateless_continuation_appends_status_after_preserved_items() {
    let server = common::FixtureServer::start(|_attempt, _head| {
        common::sse_fixture("openai_responses", "stored_completed.sse")
    })
    .await;
    let adapter = rustx::model::OpenAiResponsesAdapter::new(
        rustx::model::OpenAiAdapterConfig::new("test-key")
            .with_api_base(server.url("/v1"))
            .with_responses_storage(rustx::model::ResponsesStorageMode::Stateless),
    );
    let preserved = serde_json::json!({
        "type": "function_call",
        "call_id": "call_01",
        "name": "alpha",
        "arguments": "{}",
    });
    let mut request = status_request(
        ModelProtocol::OpenAiResponses,
        vec![
            agent("attempt-1-agent-1", vec![text_block("previous turn")]),
            inbound_user("msg-b", "actually do not deploy it"),
        ],
    );
    request.continuation = Some(ProviderContinuationState::OpenAiResponses(
        OpenAiResponsesContinuation::Stateless {
            items: vec![preserved.clone()],
        },
    ));
    let events = common::collect_events(&adapter, request).await;
    assert!(matches!(events.last(), Some(ModelEvent::Completed { .. })));
    let body: serde_json::Value =
        serde_json::from_str(&server.request_body(0)).expect("request body is JSON");
    let input = body["input"].as_array().expect("input items");
    assert_eq!(
        input[0], preserved,
        "preserved provider-native output items are replayed first"
    );
    let user_item = input
        .iter()
        .find(|item| item["type"] == "message" && item["role"] == "user")
        .expect("the user tail is transmitted");
    let content = user_item["content"].as_array().expect("content units");
    assert_eq!(
        content.last().expect("status unit")["text"],
        STATUS_TEXT,
        "the status is the final input_text unit of the target user item"
    );
    assert_eq!(
        content[0]["text"], "actually do not deploy it",
        "B's existing content stays first"
    );
    assert_eq!(
        count_text_values(&body, STATUS_TEXT),
        1,
        "the status appears exactly once in the stateless tail"
    );
}

#[tokio::test]
async fn anthropic_appends_status_without_breaking_tool_result_grouping() {
    let server = common::FixtureServer::start(|_attempt, _head| {
        common::sse_fixture("anthropic", "text.sse")
    })
    .await;
    let adapter = rustx::model::AnthropicMessagesAdapter::new(
        rustx::model::AnthropicAdapterConfig::new("test-key").with_api_base(server.url("")),
    );
    let mut request = status_request(
        ModelProtocol::AnthropicMessages,
        vec![
            inbound_user("msg-u0", "start"),
            agent(
                "attempt-1-agent-1",
                vec![
                    text_block("calling"),
                    AgentContentBlock::ToolCall(ToolCall {
                        id: ToolCallId::new("call-1"),
                        tool_id: ToolId::new("tool-alpha"),
                        name: "alpha".to_owned(),
                        arguments: serde_json::json!({}),
                    }),
                ],
            ),
            MessageBlock::Tool(ToolMessageBlock {
                id: MessageId::new("msg-tool-1"),
                tool_call_id: ToolCallId::new("call-1"),
                tool_id: ToolId::new("tool-alpha"),
                result: ToolExecutionResult {
                    status: ToolExecutionStatus::Success,
                    content: vec![rustx::tools::types::ToolResultContent::Text(TextBlock {
                        text: "ok".to_owned(),
                    })],
                    duration_ms: 1,
                    exit_code: Some(0),
                    artifacts: Vec::new(),
                    truncation: None,
                },
            }),
            inbound_user("msg-a", "deploy it"),
            inbound_user("msg-b", "actually do not deploy it"),
        ],
    );
    request.tools = vec![common::tool("alpha", "tool-alpha")];
    let events = common::collect_events(&adapter, request).await;
    assert!(matches!(events.last(), Some(ModelEvent::Completed { .. })));
    let body: serde_json::Value =
        serde_json::from_str(&server.request_body(0)).expect("request body is JSON");
    let messages = body["messages"].as_array().expect("messages array");
    // The tool result forms its own user message directly after the
    // assistant message, never polluted by the status.
    let tool_result_message = messages
        .iter()
        .find(|message| {
            message["content"]
                .as_array()
                .is_some_and(|content| content[0]["type"] == "tool_result")
        })
        .expect("tool result user message");
    assert!(
        !tool_result_message
            .to_string()
            .contains("<system-reminder>"),
        "the status must never sit between assistant tool_use and its tool_result"
    );
    // The status is the final text block of the target fresh user message.
    let target_message = messages
        .iter()
        .find(|message| {
            message["content"].as_array().is_some_and(|content| {
                content
                    .iter()
                    .any(|block| block["text"] == "actually do not deploy it")
            })
        })
        .expect("target user message");
    let content = target_message["content"]
        .as_array()
        .expect("content blocks");
    assert_eq!(content[0]["text"], "actually do not deploy it");
    assert_eq!(
        content.last().expect("status block")["text"],
        STATUS_TEXT,
        "the status is the final text block of the target user message"
    );
    assert_eq!(
        count_text_values(&body, STATUS_TEXT),
        1,
        "the status appears exactly once on the wire"
    );
}
