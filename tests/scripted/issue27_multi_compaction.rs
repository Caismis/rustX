//! Issue #27: deterministic multi-compaction validation through the final
//! durable `ConversationRuntime` path.
//!
//! Every test here drives at least two committed compactions through the
//! production `ConversationRuntime` → `AgentExecution` → `ContextRuntime`
//! pipeline
//! — never against the context engine or `ConversationState` in isolation —
//! and asserts the Issue #27 contract byte for byte:
//!
//! - the Message Ledger retains everything, including both compaction
//!   summaries and every retired message, while each rebuilt request
//!   excludes exactly the retired span (no resurrection, no double summary);
//! - every actual primary request is reconstructible from its frozen
//!   Request Snapshot plus the historical Surface — including requests taken
//!   *before* later compactions rewrote the active surface;
//! - the second compaction operates on the already-compacted surface: its
//!   summary span retires the still-active first summary;
//! - provider continuation never leaks across an attempt boundary,
//!   propagates exactly within an attempt, and is invalidated exactly once
//!   by a committed compaction, with the continuation-owning tool unit
//!   retired completely;
//! - client attach/detach is projection ownership only: compactions commit
//!   while no client is attached, canonical reads answer while detached, and
//!   a fresh attachment replays the continuous history;
//! - under `summaryModel.mode = "session"`, a `model_set` linearized while
//!   an attempt is parked mid-turn cannot change that attempt's summary
//!   model (the session-mode complement of Issue #42's explicit-mode freeze
//!   test).
//!
//! Every ordering is established by an explicit synchronization point —
//! watch channels, observation-stream predicates, exact-condition waits —
//! never by a delay.

use super::{common, support};

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use rustx::context::{
    AgentStatusClock, AgentStatusComposer, AgentStatusFact, AgentStatusRenderContext,
    AgentStatusSectionId, AgentStatusSectionProvider, ContextError, SessionContextPolicy,
};
use rustx::message::content::TextBlock;
use rustx::message::types::{
    AssistantContentBlock, ContentBlockIndex, ContextKind, InboundKind, MessageBlock,
    UserContentBlock,
};
use rustx::model::catalog::ModelRef;
use rustx::model::session::{SessionModelConfig, SessionModelState, SummaryModelPolicy};
use rustx::model::snapshot::RequestSnapshot;
use rustx::model::types::ModelRequest;
use rustx::model::{
    ModelAdapter, ModelError, ModelErrorKind, ModelEvent, ModelFinishReason, ModelProtocol,
    RequestParams,
};
use rustx::runtime::continuation::{
    AnthropicContinuation, OpenAiResponsesContinuation, ProviderContinuationState,
};
use rustx::runtime_client::types::{RequestId, RuntimeClientRequest};
use rustx::runtime_client::{
    EventDelivery, EventSubscription, RUNTIME_CLIENT_PROTOCOL_VERSION, RuntimeAttachment,
    RuntimeClientCursor, RuntimeClientError, RuntimeClientEvent, RuntimeClientHost,
    RuntimeClientProtocolEvent,
};
use rustx::tools::executor::ToolRegistry;
use support::fake::{
    FakeModel, FakeStep, FakeTool, ScriptedCall, model_release, success_result, tool_call_events,
};
use support::model::{
    FixtureModel, ScriptedAdapterFactory, fixture_registry, scripted_session_model,
};
use support::runtime_client_fixture::RuntimeClientFixture;

/// The outer liveness guard: waiting is exact (watch channels and the
/// observation stream), so this only bounds a pathological stall.
const LIVENESS: Duration = Duration::from_secs(120);

/// A fixed deterministic status clock.
#[derive(Debug, Clone, Copy)]
struct FixedStatusClock;

impl AgentStatusClock for FixedStatusClock {
    fn now(&self) -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::parse_from_rfc3339("2026-08-07T12:00:00Z")
            .expect("fixed clock")
            .with_timezone(&chrono::Utc)
    }
}

/// An Agent Status extension provider whose rendered value counts its own
/// invocations, so a test proves exactly how many times status was composed:
/// once per fresh-inbound step, never on an overflow retry.
struct CountingStatusProvider {
    calls: Arc<AtomicU64>,
}

impl AgentStatusSectionProvider for CountingStatusProvider {
    fn section_id(&self) -> AgentStatusSectionId {
        AgentStatusSectionId::new("issue27-probe")
    }

    fn section(
        &self,
        _context: &AgentStatusRenderContext,
    ) -> Result<Option<Vec<AgentStatusFact>>, ContextError> {
        let sample = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        Ok(Some(vec![AgentStatusFact {
            label: "probe".to_owned(),
            value: format!("sample-{sample}"),
        }]))
    }
}

/// Builds the status composer carrying the counting probe provider.
fn counting_composer(calls: Arc<AtomicU64>) -> AgentStatusComposer {
    let mut composer = AgentStatusComposer::new(Arc::new(FixedStatusClock));
    composer
        .register(Arc::new(CountingStatusProvider { calls }))
        .expect("the probe section id is free");
    composer
}

/// Receives from the observation stream until the predicate matches.
async fn receive_until(
    subscription: &EventSubscription,
    mut predicate: impl FnMut(&RuntimeClientProtocolEvent) -> bool,
) -> Vec<RuntimeClientProtocolEvent> {
    tokio::time::timeout(LIVENESS, async {
        let mut seen = Vec::new();
        loop {
            let EventDelivery::Event(event) = subscription.next().await else {
                panic!("the subscription must stay open and contiguous");
            };
            let matched = predicate(&event);
            seen.push(event);
            if matched {
                return seen;
            }
        }
    })
    .await
    .expect("the observation stream must not stall")
}

/// Whether one delivered event is the terminal attempt settlement.
fn settled(event: &RuntimeClientProtocolEvent) -> bool {
    matches!(event.event, RuntimeClientEvent::AttemptSettled { .. })
}

/// The committed compaction generations carried by `ContextCompacted`
/// events, in delivery order.
fn compaction_generations(events: &[RuntimeClientProtocolEvent]) -> Vec<u64> {
    events
        .iter()
        .filter_map(|event| match &event.event {
            RuntimeClientEvent::ContextCompacted { context, .. } => context
                .latest_compaction
                .as_ref()
                .map(|view| view.generation),
            _ => None,
        })
        .collect()
}

fn text(value: &str) -> Vec<UserContentBlock> {
    vec![UserContentBlock::Text(TextBlock {
        text: value.to_owned(),
    })]
}

/// One scripted model invocation emitting `value` and completing with Stop.
fn turn_text(value: String) -> Vec<FakeStep> {
    vec![
        FakeStep::Emit(ModelEvent::Started),
        FakeStep::Emit(ModelEvent::TextDelta {
            block_index: ContentBlockIndex::new(0),
            text: value,
        }),
        FakeStep::Emit(ModelEvent::Completed {
            finish_reason: ModelFinishReason::Stop,
            usage: None,
        }),
    ]
}

/// One scripted invocation that fails immediately with a provider-reported
/// context-window overflow.
fn overflow_turn() -> Vec<FakeStep> {
    vec![
        FakeStep::Emit(ModelEvent::Started),
        FakeStep::Emit(ModelEvent::Failed {
            error: ModelError {
                kind: ModelErrorKind::ContextWindowExceeded,
                message: "context window exceeded".to_owned(),
                retry_after_ms: None,
                provider_code: None,
            },
        }),
    ]
}

/// Attaches one client and subscribes it to the full observation stream.
fn attach(host: &RuntimeClientHost) -> (RuntimeAttachment, EventSubscription) {
    let attachment = host
        .attach(RUNTIME_CLIENT_PROTOCOL_VERSION)
        .expect("attach")
        .0;
    let subscription = attachment
        .subscribe_events(RuntimeClientCursor::new(0))
        .expect("subscribe");
    (attachment, subscription)
}

/// Submits one inbound message through the attachment control channel.
fn submit(attachment: &RuntimeAttachment, id: u64, value: &str) {
    let response = attachment.handle_request(RuntimeClientRequest::SubmitInbound {
        id: RequestId::new(id),
        content: text(value),
    });
    assert!(
        response.error.is_none(),
        "submit must be accepted: {response:?}"
    );
}

/// Waits for the durable request-fact read to expose the expected count.
async fn await_request_history_len(host: &RuntimeClientHost, expected: usize) {
    tokio::time::timeout(LIVENESS, async {
        loop {
            if common::request_snapshots(&host.request_history()).len() == expected {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("request history transfer must settle");
}

/// Waits until the runtime is idle again and returns its current working
/// Message Ledger projection.
async fn await_ledger(host: &RuntimeClientHost) -> Vec<MessageBlock> {
    tokio::time::timeout(LIVENESS, async {
        loop {
            if let Some(ledger) = host.host_ledger() {
                return ledger;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the runtime must re-own the conversation state after settlement")
}

/// Reconstructs one retained snapshot by position in an on-demand durable
/// read result.
fn reconstruct_at(host: &RuntimeClientHost, index: usize) -> (RequestSnapshot, ModelRequest) {
    let history = host.request_history();
    let snapshot = common::request_snapshots(&history)[index].clone();
    let request = host
        .reconstruct_request(&snapshot.identity)
        .expect("retained request reconstructs");
    (snapshot, request)
}

/// Serializes canonical messages for byte-content marker assertions.
fn wire(messages: &[MessageBlock]) -> String {
    serde_json::to_string(messages).expect("canonical messages serialize")
}

/// The number of committed compaction-summary messages in one message list.
fn compaction_summaries(messages: &[MessageBlock]) -> usize {
    messages
        .iter()
        .filter(|message| {
            matches!(
                message,
                MessageBlock::User(user) if user.kind == InboundKind::CompactionSummary
            )
        })
        .count()
}

/// Whether one message is an admitted Agent Status runtime-context fact.
fn is_agent_status(message: &MessageBlock) -> bool {
    matches!(
        message,
        MessageBlock::User(user)
            if user.kind == InboundKind::Context(ContextKind::AgentStatus)
    )
}

/// Every tool call in one message list has its result, every result follows
/// its call, and neither ever appears without the other.
fn assert_tool_units_complete(messages: &[MessageBlock], context: &str) {
    let mut calls: Vec<String> = Vec::new();
    let mut results: Vec<String> = Vec::new();
    for message in messages {
        match message {
            MessageBlock::Assistant(assistant) => {
                for block in &assistant.content {
                    if let AssistantContentBlock::ToolCall(call) = block {
                        calls.push(call.id.as_str().to_owned());
                    }
                }
            }
            MessageBlock::Tool(tool) => {
                let id = tool.tool_call_id.as_str().to_owned();
                assert!(
                    calls.contains(&id),
                    "tool result {id} without its preceding call in {context}"
                );
                results.push(id);
            }
            MessageBlock::User(_) => {}
        }
    }
    for id in &calls {
        assert!(
            results.contains(id),
            "tool call {id} without its result in {context}"
        );
    }
}

fn anthropic_state() -> ProviderContinuationState {
    ProviderContinuationState::Anthropic(AnthropicContinuation {
        opaque: serde_json::json!({"signature": "sig-27"}),
    })
}

fn stored_state() -> ProviderContinuationState {
    ProviderContinuationState::OpenAiResponses(OpenAiResponsesContinuation::Stored {
        previous_response_id: "resp_27".to_owned(),
    })
}

// ---------------------------------------------------------------------------
// Repeated proactive compaction
// ---------------------------------------------------------------------------

/// **Repeated proactive compaction through the runtime.**
///
/// Three attempts over an explicit-summary session model with a small
/// primary window: attempts 2 and 3 each commit exactly one proactive
/// compaction before admission. The test asserts the whole Issue #27
/// canonical-evidence contract: distinct frozen snapshots R1/R2/R3 with
/// strictly advancing Surface revisions, byte-exact historical
/// reconstruction of every request — including R1 and R2 *after* the second
/// compaction rewrote the active surface — ledger retention of everything,
/// and exclusion of retired spans from every rebuilt request.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn repeated_proactive_compaction_preserves_canonical_evidence_through_the_runtime() {
    // Soft input limit: 4096 (window) - 512 (reserve) - 128 (output) = 3456
    // tokens ≈ 13_824 serialized bytes. One 16_000-byte assistant filler
    // pushes the next attempt's baseline projection past it.
    const WINDOW: u64 = 4_096;
    const OUTPUT: u32 = 128;
    const RESERVE: u64 = 512;
    const FILLER_BYTES: usize = 16_000;

    let filler_one = format!("FILLER-ONE-MARKER {}", "x".repeat(FILLER_BYTES));
    let filler_two = format!("FILLER-TWO-MARKER {}", "y".repeat(FILLER_BYTES));

    let adapter: Arc<FakeModel> = Arc::new(FakeModel::new(vec![
        turn_text(filler_one.clone()),                              // R1
        turn_text("SUMMARY-ONE covers filler one".to_owned()),      // compaction #1
        turn_text(filler_two.clone()),                              // R2
        turn_text("SUMMARY-TWO supersedes summary one".to_owned()), // compaction #2
        turn_text("final answer".to_owned()),                       // R3
    ]));
    let primary = FixtureModel::text(
        "fixture/primary-model",
        ModelProtocol::OpenAiChatCompletions,
    )
    .with_context_window(WINDOW)
    .with_max_output_tokens(OUTPUT);
    let summary = FixtureModel::text(
        "fixture/summary-model",
        ModelProtocol::OpenAiChatCompletions,
    )
    .with_context_window(100_000)
    .with_max_output_tokens(256);
    let registry = fixture_registry(
        &[primary, summary],
        &ScriptedAdapterFactory::new(adapter.clone() as Arc<dyn ModelAdapter>),
    );
    let session_model = SessionModelState::new(
        registry,
        SessionModelConfig {
            summary_model: SummaryModelPolicy::Explicit {
                model: ModelRef::parse("fixture/summary-model").expect("valid reference"),
                reasoning_profile: None,
                request_params: RequestParams::new(),
                max_output_tokens: None,
            },
            ..SessionModelConfig::of(
                ModelRef::parse("fixture/primary-model").expect("valid reference"),
            )
        },
    )
    .expect("the session model resolves");

    let status_calls = Arc::new(AtomicU64::new(0));
    let fixture = RuntimeClientFixture::builder("conv-27-proactive")
        .session_model(session_model)
        .context_policy(SessionContextPolicy {
            reserve_tokens: RESERVE,
            keep_recent_tokens: 0,
            summary_output_cap: None,
        })
        .composer(counting_composer(status_calls.clone()))
        .build()
        .await;
    let host = &fixture.host;
    let (attachment, subscription) = attach(host);

    // Attempt 1: the small request never compacts.
    submit(&attachment, 1, "turn one");
    receive_until(&subscription, settled).await;
    await_request_history_len(host, 1).await;
    let (_, r1_early) = reconstruct_at(host, 0);

    // Attempt 2 commits compaction #1 proactively, before admission.
    submit(&attachment, 2, "turn two");
    let events_two = receive_until(&subscription, settled).await;
    await_request_history_len(host, 2).await;
    // R1 and R2 both reconstruct byte-exactly *after* compaction #1.
    let (_, r1_mid) = reconstruct_at(host, 0);
    let (_, r2_mid) = reconstruct_at(host, 1);

    // Attempt 3 commits compaction #2 on the already-compacted surface.
    submit(&attachment, 3, "turn three");
    let events_three = receive_until(&subscription, settled).await;
    await_request_history_len(host, 3).await;

    // Exactly five actual model invocations: three primaries, two summaries.
    let requests = adapter.requests();
    assert_eq!(requests.len(), 5, "three primaries plus two summaries");
    assert_eq!(requests[0].model(), "primary-model");
    assert_eq!(requests[1].model(), "summary-model");
    assert_eq!(requests[2].model(), "primary-model");
    assert_eq!(requests[3].model(), "summary-model");
    assert_eq!(requests[4].model(), "primary-model");

    // Summary requests are the canonical one-off shape: the resolved summary
    // invocation, no tools, no system prompt, no continuation, and the
    // deterministic retired-span input.
    for summary_request in [&requests[1], &requests[3]] {
        assert!(summary_request.tools.is_empty());
        assert!(summary_request.effective_system_prompt.is_empty());
        assert_eq!(summary_request.continuation, None);
    }
    assert!(wire(&requests[1].messages).contains("FILLER-ONE-MARKER"));
    let summary_two_input = wire(&requests[3].messages);
    assert!(
        summary_two_input.contains("SUMMARY-ONE covers filler one"),
        "the second compaction's span retired the still-active first summary"
    );
    assert!(summary_two_input.contains("FILLER-TWO-MARKER"));

    // R1 observed the pre-compaction surface; R2/R3 observe exactly the
    // rebuilt surfaces [sum, inbound, status].
    let r1_wire = wire(&requests[0].messages);
    assert!(r1_wire.contains("turn one"));
    assert!(r1_wire.contains("sample-1"));
    assert_eq!(compaction_summaries(&requests[0].messages), 0);

    assert_eq!(requests[2].messages.len(), 3, "[sum1, in2, st2]");
    assert_eq!(compaction_summaries(&requests[2].messages), 1);
    let r2_wire = wire(&requests[2].messages);
    assert!(r2_wire.contains("SUMMARY-ONE covers filler one"));
    assert!(
        !r2_wire.contains("FILLER-ONE-MARKER"),
        "the retired filler never appears in a rebuilt request"
    );
    assert!(r2_wire.contains("turn two"));
    assert!(r2_wire.contains("sample-2"));
    assert!(
        !r2_wire.contains("sample-1"),
        "the retired status fact stays retired"
    );

    assert_eq!(requests[4].messages.len(), 3, "[sum2, in3, st3]");
    assert_eq!(compaction_summaries(&requests[4].messages), 1);
    let r3_wire = wire(&requests[4].messages);
    assert!(r3_wire.contains("SUMMARY-TWO supersedes summary one"));
    assert!(
        !r3_wire.contains("SUMMARY-ONE covers filler one"),
        "the retired first summary never resurrects beside the second"
    );
    assert!(!r3_wire.contains("FILLER-ONE-MARKER"));
    assert!(!r3_wire.contains("FILLER-TWO-MARKER"));
    assert!(!r3_wire.contains("sample-1"));
    assert!(!r3_wire.contains("sample-2"));
    assert!(r3_wire.contains("turn three"));
    assert!(r3_wire.contains("sample-3"));

    // Three distinct frozen snapshots with strictly advancing Surface
    // revisions, one per attempt, never a retry.
    let history = host.request_history();
    let snapshots = common::request_snapshots(&history);
    assert_eq!(snapshots.len(), 3);
    assert!(snapshots.iter().all(|s| s.identity.retry_number == 0));
    let attempt_ids: Vec<&str> = snapshots
        .iter()
        .map(|s| s.identity.attempt_id.as_str())
        .collect();
    assert_eq!(
        attempt_ids,
        [
            "conv-27-proactive-attempt-0",
            "conv-27-proactive-attempt-1",
            "conv-27-proactive-attempt-2"
        ]
    );
    assert!(snapshots[0].surface_revision < snapshots[1].surface_revision);
    assert!(snapshots[1].surface_revision < snapshots[2].surface_revision);

    // Historical reconstruction is byte-exact and stable across the second
    // compaction: R1 and R2 reconstruct identically before and after it.
    let (_, r1_late) = reconstruct_at(host, 0);
    let (_, r2_late) = reconstruct_at(host, 1);
    let (_, r3_late) = reconstruct_at(host, 2);
    assert_eq!(r1_early, r1_mid);
    assert_eq!(r1_mid, r1_late);
    assert_eq!(r2_mid, r2_late);
    assert_eq!(r1_late, requests[0]);
    assert_eq!(r2_late, requests[2]);
    assert_eq!(r3_late, requests[4]);

    // The Message Ledger retains everything; the active surface exclusions
    // above came from Surface revisions, never from destructive rewrites.
    // Commit order per attempt: the fresh inbound commits at turn start, the
    // proactive compaction's summary commits before status admission, and
    // the assistant message commits at turn completion.
    let ledger = await_ledger(host).await;
    assert_eq!(
        ledger.len(),
        11,
        "3 inbounds + 3 status facts + 3 assistants + 2 summaries"
    );
    assert!(matches!(&ledger[0], MessageBlock::User(u) if u.kind == InboundKind::Message));
    assert!(is_agent_status(&ledger[1]));
    assert!(matches!(&ledger[2], MessageBlock::Assistant(_)));
    assert!(matches!(&ledger[3], MessageBlock::User(u) if u.kind == InboundKind::Message));
    assert!(
        matches!(&ledger[4], MessageBlock::User(u) if u.kind == InboundKind::CompactionSummary)
    );
    assert!(is_agent_status(&ledger[5]));
    assert!(matches!(&ledger[6], MessageBlock::Assistant(_)));
    assert!(matches!(&ledger[7], MessageBlock::User(u) if u.kind == InboundKind::Message));
    assert!(
        matches!(&ledger[8], MessageBlock::User(u) if u.kind == InboundKind::CompactionSummary)
    );
    assert!(is_agent_status(&ledger[9]));
    assert!(matches!(&ledger[10], MessageBlock::Assistant(_)));
    let ledger_wire = wire(&ledger);
    assert!(
        ledger_wire.contains("FILLER-ONE-MARKER"),
        "the ledger retains the retired filler verbatim"
    );
    assert!(ledger_wire.contains("FILLER-TWO-MARKER"));
    assert!(ledger_wire.contains("SUMMARY-ONE covers filler one"));
    assert!(ledger_wire.contains("sample-1"));

    // The external projection reports exactly two committed compactions in
    // generation order, attributed to the attempts that committed them.
    let compacted: Vec<(String, u64)> = events_two
        .iter()
        .chain(&events_three)
        .filter_map(|event| match &event.event {
            RuntimeClientEvent::ContextCompacted {
                attempt_id,
                context,
            } => context.latest_compaction.as_ref().and_then(|view| {
                attempt_id
                    .as_ref()
                    .map(|attempt_id| (attempt_id.as_str().to_owned(), view.generation))
            }),
            _ => None,
        })
        .collect();
    assert_eq!(
        compacted,
        [
            ("conv-27-proactive-attempt-1".to_owned(), 1),
            ("conv-27-proactive-attempt-2".to_owned(), 2)
        ]
    );
    let (snapshot, _) = host.snapshot().expect("snapshot");
    assert_eq!(snapshot.context.compaction_count, 2);
    assert_eq!(
        snapshot
            .context
            .latest_compaction
            .as_ref()
            .expect("a latest compaction view")
            .generation,
        2
    );

    // Status was composed exactly once per fresh-inbound step.
    assert_eq!(status_calls.load(Ordering::SeqCst), 3);

    // The session model was never contaminated by summary traffic.
    let view = fixture.runtime.model_view();
    assert_eq!(
        view.configured,
        SessionModelConfig {
            summary_model: SummaryModelPolicy::Explicit {
                model: ModelRef::parse("fixture/summary-model").expect("valid reference"),
                reasoning_profile: None,
                request_params: RequestParams::new(),
                max_output_tokens: None,
            },
            ..SessionModelConfig::of(
                ModelRef::parse("fixture/primary-model").expect("valid reference"),
            )
        }
    );
}

// ---------------------------------------------------------------------------
// Repeated overflow compaction: continuation + tool-unit integrity
// ---------------------------------------------------------------------------

/// **Repeated overflow compaction with continuation and tool turns.**
///
/// Two attempts each overflow exactly once and compact-and-retry, with a
/// multi-turn tool batch in between. The test asserts: continuation never
/// crosses an attempt boundary, propagates exactly within an attempt into
/// the overflowed request, and is invalidated exactly once by the committed
/// compaction; the continuation-owning tool unit retires completely (no
/// orphan call, no orphan result) and lands complete inside the summary
/// input; the second compaction's span is exactly the still-active first
/// summary; and the overflow retry reuses its step's admitted context
/// generation while rebuilding from its own post-compaction Surface
/// revision.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn repeated_overflow_compaction_invalidates_continuation_once_and_retires_complete_tool_units()
 {
    const CONVERSATION: &str = "conv-27-overflow";
    let call = ScriptedCall {
        id: "call-1",
        tool_id: "tool-alpha",
        name: "alpha",
        arguments: serde_json::json!({}),
    };
    let cross_attempt = stored_state();
    let intra_attempt = anthropic_state();

    // Block 0 carries the continuation annotation; the tool call is block 1
    // (the exact layout proven by the m4 continuation suite).
    let call_events = tool_call_events(1, &call);
    let adapter = Arc::new(FakeModel::new(vec![
        // Attempt 1 completes holding continuation state; it must not leak.
        vec![
            FakeStep::Emit(ModelEvent::Started),
            FakeStep::Emit(ModelEvent::ContinuationState {
                block_index: ContentBlockIndex::new(0),
                state: cross_attempt.clone(),
            }),
            FakeStep::Emit(ModelEvent::TextDelta {
                block_index: ContentBlockIndex::new(1),
                text: "first".to_owned(),
            }),
            FakeStep::Emit(ModelEvent::Completed {
                finish_reason: ModelFinishReason::Stop,
                usage: None,
            }),
        ],
        // Attempt 2 turn 1: a tool-call turn carrying continuation state.
        vec![
            FakeStep::Emit(ModelEvent::Started),
            FakeStep::Emit(ModelEvent::ContinuationState {
                block_index: ContentBlockIndex::new(0),
                state: intra_attempt.clone(),
            }),
            FakeStep::Emit(call_events[0].clone()),
            FakeStep::Emit(call_events[1].clone()),
            FakeStep::Emit(call_events[2].clone()),
            FakeStep::Emit(ModelEvent::Completed {
                finish_reason: ModelFinishReason::ToolCalls,
                usage: None,
            }),
        ],
        // Attempt 2 turn 2 request 0: scripted provider overflow.
        overflow_turn(),
        // Compaction #1 (must cover the continuation-owning tool unit).
        turn_text("SUMMARY-ONE covers the tool turn".to_owned()),
        // Attempt 2 turn 2 retry.
        turn_text("recovered after one".to_owned()),
        // Attempt 3 turn 1 request 0: scripted provider overflow.
        overflow_turn(),
        // Compaction #2: its span is exactly the still-active first summary.
        turn_text("SUMMARY-TWO supersedes summary one".to_owned()),
        // Attempt 3 turn 1 retry.
        turn_text("recovered after two".to_owned()),
    ]));
    let status_calls = Arc::new(AtomicU64::new(0));
    let mut tools = ToolRegistry::new();
    FakeTool::new(
        common::tool("alpha", "tool-alpha"),
        success_result("TOOL-RESULT-OK"),
    )
    .register(&mut tools);
    let fixture = RuntimeClientFixture::builder(CONVERSATION)
        .tools(tools)
        .session_model(scripted_session_model(
            adapter.clone() as Arc<dyn ModelAdapter>
        ))
        .composer(counting_composer(status_calls.clone()))
        .build()
        .await;
    let host = &fixture.host;
    let (attachment, subscription) = attach(host);

    let mut events = Vec::new();
    submit(&attachment, 1, "turn one");
    events.extend(receive_until(&subscription, settled).await);
    await_request_history_len(host, 1).await;

    submit(&attachment, 2, "turn two");
    events.extend(receive_until(&subscription, settled).await);
    await_request_history_len(host, 4).await;

    submit(&attachment, 3, "turn three");
    events.extend(receive_until(&subscription, settled).await);
    await_request_history_len(host, 6).await;

    let requests = adapter.requests();
    assert_eq!(requests.len(), 8, "six primaries plus two summaries");

    // The continuation contract.
    assert_eq!(
        requests[0].continuation, None,
        "the first request of a conversation never fabricates continuation"
    );
    assert_eq!(
        requests[1].continuation, None,
        "attempt 1's continuation never leaks across the attempt boundary"
    );
    assert_eq!(
        requests[2].continuation,
        Some(intra_attempt.clone()),
        "the overflowed request carried the attempt's exact opaque continuation"
    );
    assert_eq!(
        requests[3].continuation, None,
        "a summary request never carries continuation"
    );
    assert_eq!(
        requests[4].continuation, None,
        "the committed compaction invalidated the continuation exactly once"
    );
    assert_eq!(requests[5].continuation, None);
    assert_eq!(requests[6].continuation, None);
    assert_eq!(requests[7].continuation, None);

    // Tool definitions reached the tool turn; the summary stays one-off.
    assert_eq!(requests[1].tools.len(), 1);
    for summary_request in [&requests[3], &requests[6]] {
        assert!(summary_request.tools.is_empty());
        assert!(summary_request.effective_system_prompt.is_empty());
    }

    // No request ever carries half a tool unit.
    for (index, request) in requests.iter().enumerate() {
        assert_tool_units_complete(&request.messages, &format!("request {index}"));
    }

    // Compaction #1 retired the continuation-owning tool unit completely:
    // the retry observes only the summary — neither the call nor its result.
    let retry_two = wire(&requests[4].messages);
    assert_eq!(requests[4].messages.len(), 1, "[sum1]");
    assert!(retry_two.contains("SUMMARY-ONE covers the tool turn"));
    assert!(!retry_two.contains("call-1"));
    assert!(!retry_two.contains("TOOL-RESULT-OK"));

    // The summary input carried the complete retired tool unit.
    let summary_one = wire(&requests[3].messages);
    assert!(summary_one.contains("call-1"));
    assert!(summary_one.contains("TOOL-RESULT-OK"));

    // Compaction #2's span was exactly the still-active first summary.
    let summary_two = wire(&requests[6].messages);
    assert!(
        summary_two.contains("SUMMARY-ONE covers the tool turn"),
        "the second compaction retired the first summary, nothing else"
    );

    // The second retry observes [sum2, in3, st3]: the new summary, the fresh
    // inbound, and its admitted status fact — nothing retired.
    let retry_three = wire(&requests[7].messages);
    assert_eq!(requests[7].messages.len(), 3, "[sum2, in3, st3]");
    assert!(retry_three.contains("SUMMARY-TWO supersedes summary one"));
    assert!(!retry_three.contains("SUMMARY-ONE covers the tool turn"));
    assert!(!retry_three.contains("call-1"));
    assert!(!retry_three.contains("TOOL-RESULT-OK"));
    assert!(retry_three.contains("turn three"));
    assert!(retry_three.contains("sample-3"));

    // Six frozen snapshots: att1×1, att2×3 (tool turn + overflow + retry),
    // att3×2 (overflow + retry).
    let history = host.request_history();
    let snapshots = common::request_snapshots(&history);
    assert_eq!(snapshots.len(), 6);
    let attempt = |index: usize| snapshots[index].identity.attempt_id.as_str();
    assert_eq!(attempt(0), "conv-27-overflow-attempt-0");
    assert_eq!(attempt(1), "conv-27-overflow-attempt-1");
    assert_eq!(attempt(2), "conv-27-overflow-attempt-1");
    assert_eq!(attempt(3), "conv-27-overflow-attempt-1");
    assert_eq!(attempt(4), "conv-27-overflow-attempt-2");
    assert_eq!(attempt(5), "conv-27-overflow-attempt-2");
    assert_eq!(snapshots[2].identity.retry_number, 0);
    assert_eq!(snapshots[3].identity.retry_number, 1);
    assert_eq!(snapshots[4].identity.retry_number, 0);
    assert_eq!(snapshots[5].identity.retry_number, 1);

    // Each overflow pair is one turn: the retry reuses the admitted context
    // generation (contributors never re-ran) but rebuilds from its own
    // post-compaction Surface revision.
    assert_eq!(snapshots[2].identity.turn, snapshots[3].identity.turn);
    assert_eq!(
        snapshots[2].context_generation, snapshots[3].context_generation,
        "the overflow retry reuses the one admitted context generation"
    );
    assert_ne!(
        snapshots[1].context_generation, snapshots[2].context_generation,
        "a new primary step admits a new context generation"
    );
    assert_ne!(
        snapshots[2].surface_revision, snapshots[3].surface_revision,
        "compaction gives the retry its own historical Surface revision"
    );
    assert_eq!(snapshots[4].identity.turn, snapshots[5].identity.turn);
    assert_eq!(
        snapshots[4].context_generation,
        snapshots[5].context_generation
    );
    assert_ne!(snapshots[4].surface_revision, snapshots[5].surface_revision);

    // The frozen snapshots carry the exact per-request continuation facts.
    assert_eq!(snapshots[2].continuation, Some(intra_attempt));
    assert_eq!(snapshots[3].continuation, None);
    assert_eq!(snapshots[5].continuation, None);

    // Every actual primary request reconstructs byte-exactly after both
    // compactions committed.
    let expected = [
        &requests[0],
        &requests[1],
        &requests[2],
        &requests[4],
        &requests[5],
        &requests[7],
    ];
    for (snapshot, request) in snapshots.iter().zip(expected) {
        assert_eq!(
            host.reconstruct_request(&snapshot.identity)
                .expect("settled historical request reconstructs"),
            *request
        );
    }

    // The ledger retains the retired tool unit and both summaries verbatim.
    let ledger = await_ledger(host).await;
    assert_eq!(ledger.len(), 13);
    assert!(
        matches!(&ledger[5], MessageBlock::Assistant(assistant)
            if assistant.content.iter().any(|block| matches!(block, AssistantContentBlock::ToolCall(_)))),
        "the retired continuation-owning tool call stays in the ledger"
    );
    assert!(matches!(&ledger[6], MessageBlock::Tool(_)));
    assert_eq!(compaction_summaries(&ledger), 2);
    let ledger_wire = wire(&ledger);
    assert!(ledger_wire.contains("call-1"));
    assert!(ledger_wire.contains("TOOL-RESULT-OK"));
    assert!(ledger_wire.contains("SUMMARY-ONE covers the tool turn"));
    assert!(ledger_wire.contains("SUMMARY-TWO supersedes summary one"));

    // Exactly two committed compactions, attributed in generation order.
    let compacted: Vec<(String, u64)> = events
        .iter()
        .filter_map(|event| match &event.event {
            RuntimeClientEvent::ContextCompacted {
                attempt_id,
                context,
            } => context.latest_compaction.as_ref().and_then(|view| {
                attempt_id
                    .as_ref()
                    .map(|attempt_id| (attempt_id.as_str().to_owned(), view.generation))
            }),
            _ => None,
        })
        .collect();
    assert_eq!(
        compacted,
        [
            ("conv-27-overflow-attempt-1".to_owned(), 1),
            ("conv-27-overflow-attempt-2".to_owned(), 2)
        ]
    );
    let (snapshot, _) = host.snapshot().expect("snapshot");
    assert_eq!(snapshot.context.compaction_count, 2);

    // Status composition ran exactly once per fresh-inbound step; the
    // post-tool turn and both retries composed nothing.
    assert_eq!(status_calls.load(Ordering::SeqCst), 3);
}

// ---------------------------------------------------------------------------
// Client ownership: detach/reattach around committed compactions
// ---------------------------------------------------------------------------

/// **Compaction and canonical truth are runtime-owned, not client-owned.**
///
/// Compaction #1 commits while *no* client is attached (the inbound was
/// admitted directly through the runtime). Canonical reads —
/// `request_history`, `reconstruct_request`, `snapshot` — answer while
/// detached. A fresh attachment gets a new projection identity and replays
/// the continuous history from cursor 0, including the compaction committed
/// in the detached window; compaction #2 then commits under the new
/// attachment with the history continuous throughout.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn compaction_and_canonical_truth_survive_client_detach_and_reattach() {
    let adapter = Arc::new(FakeModel::new(vec![
        turn_text("one".to_owned()),
        overflow_turn(),
        turn_text("SUMMARY-ONE".to_owned()),
        turn_text("two".to_owned()),
        overflow_turn(),
        turn_text("SUMMARY-TWO".to_owned()),
        turn_text("three".to_owned()),
    ]));
    let fixture = RuntimeClientFixture::builder("conv-27-ownership")
        .session_model(scripted_session_model(
            adapter.clone() as Arc<dyn ModelAdapter>
        ))
        .build()
        .await;
    let host = &fixture.host;
    let (attachment_a, subscription_a) = attach(host);
    let attachment_a_id = attachment_a.attachment_id().clone();

    // Attempt 1 under the first attachment.
    submit(&attachment_a, 1, "first");
    receive_until(&subscription_a, settled).await;
    await_request_history_len(host, 1).await;

    // Detach the only client; its handle is revoked immediately.
    attachment_a.detach();
    let rejected = attachment_a.handle_request(RuntimeClientRequest::SubmitInbound {
        id: RequestId::new(90),
        content: text("rejected"),
    });
    assert!(
        matches!(rejected.error, Some(RuntimeClientError::NotAttached)),
        "a detached handle cannot control the runtime: {rejected:?}"
    );

    // With zero attachments the runtime itself admits and settles attempt 2,
    // committing compaction #1.
    fixture
        .runtime
        .submit_inbound(text("second"))
        .expect("the runtime admits inbound without any client");
    await_request_history_len(host, 3).await;
    let detached_ledger = await_ledger(host).await;
    assert_eq!(detached_ledger.len(), 7);
    assert_eq!(compaction_summaries(&detached_ledger), 1);

    // Canonical reads answer while no client is attached.
    let requests = adapter.requests();
    assert_eq!(requests.len(), 4, "att1 + att2 overflow + summary + retry");
    let history = host.request_history();
    let snapshots = common::request_snapshots(&history);
    assert_eq!(snapshots.len(), 3);
    for (snapshot, request) in snapshots
        .iter()
        .zip([&requests[0], &requests[1], &requests[3]])
    {
        assert_eq!(
            host.reconstruct_request(&snapshot.identity)
                .expect("canonical reconstruction works while detached"),
            *request
        );
    }
    let (detached_snapshot, _) = host.snapshot().expect("snapshot without attachments");
    assert_eq!(detached_snapshot.context.compaction_count, 1);
    assert_eq!(detached_snapshot.messages.len(), 7);

    // A fresh attachment is a new projection binding over the same
    // continuous truth — never a transfer of semantic ownership.
    let (attachment_b, subscription_b) = attach(host);
    assert_ne!(attachment_b.attachment_id(), &attachment_a_id);
    // The replay from cursor 0 carries the compaction committed while no
    // client was attached. Drain the replay exactly: both settled attempts
    // are observed before any new submission, so the next `receive_until`
    // can only match the *live* settlement of attempt 3.
    let mut replayed_settlements = 0;
    let replay = receive_until(&subscription_b, |event| {
        if settled(event) {
            replayed_settlements += 1;
        }
        replayed_settlements == 2
    })
    .await;
    assert_eq!(
        compaction_generations(&replay),
        [1],
        "the replay carries the compaction committed in the detached window"
    );

    // Compaction #2 commits under the new attachment.
    submit(&attachment_b, 2, "third");
    let live = receive_until(&subscription_b, settled).await;
    await_request_history_len(host, 5).await;
    assert_eq!(compaction_generations(&live), [2]);

    let (snapshot, _) = host.snapshot().expect("snapshot");
    assert_eq!(snapshot.context.compaction_count, 2);
    assert_eq!(
        snapshot
            .context
            .latest_compaction
            .as_ref()
            .expect("a latest compaction view")
            .generation,
        2
    );
    assert_eq!(snapshot.messages.len(), 11);

    // The full history is continuous: five snapshots, all reconstructible.
    let requests = adapter.requests();
    assert_eq!(requests.len(), 7);
    let history = host.request_history();
    let snapshots = common::request_snapshots(&history);
    assert_eq!(snapshots.len(), 5);
    let expected = [
        &requests[0],
        &requests[1],
        &requests[3],
        &requests[4],
        &requests[6],
    ];
    for (snapshot, request) in snapshots.iter().zip(expected) {
        assert_eq!(
            host.reconstruct_request(&snapshot.identity)
                .expect("settled historical request reconstructs"),
            *request
        );
    }
    let ledger = await_ledger(host).await;
    assert_eq!(ledger.len(), 11);
    assert_eq!(compaction_summaries(&ledger), 2);
}

// ---------------------------------------------------------------------------
// Session-mode summary race
// ---------------------------------------------------------------------------

/// **`summaryModel.mode = "session"` freezes the attempt's summary model.**
///
/// Issue #42 proved the explicit-mode freeze; this is the session-mode
/// complement. An attempt admitted with model A parks mid-stream; a valid
/// `model_set` to B linearizes after that admission. The parked request, the
/// compaction summary it triggers, and the overflow retry all stay on A —
/// in session mode the summary model *is* the attempt's frozen primary.
/// The next admitted attempt resolves B.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn session_summary_mode_freezes_the_attempt_summary_model_against_mid_attempt_updates() {
    let (release, receiver) = model_release();
    let adapter = Arc::new(FakeModel::new(vec![
        turn_text("one".to_owned()),
        // Attempt 2 parks inside its first stream, then overflows.
        vec![
            FakeStep::Emit(ModelEvent::Started),
            FakeStep::ParkUntilReleased(receiver),
            FakeStep::Emit(ModelEvent::Failed {
                error: ModelError {
                    kind: ModelErrorKind::ContextWindowExceeded,
                    message: "context window exceeded".to_owned(),
                    retry_after_ms: None,
                    provider_code: None,
                },
            }),
        ],
        turn_text("SUMMARY-ONE".to_owned()),
        turn_text("recovered".to_owned()),
        turn_text("three".to_owned()),
    ]));
    let model_a = FixtureModel::text("fixture/model-a", ModelProtocol::OpenAiChatCompletions);
    let model_b = FixtureModel::text("fixture/model-b", ModelProtocol::OpenAiChatCompletions);
    let registry = fixture_registry(
        &[model_a, model_b],
        &ScriptedAdapterFactory::new(adapter.clone() as Arc<dyn ModelAdapter>),
    );
    let session_model = SessionModelState::new(
        registry,
        SessionModelConfig::of(ModelRef::parse("fixture/model-a").expect("valid reference")),
    )
    .expect("the session model resolves");
    let fixture = RuntimeClientFixture::builder("conv-27-race")
        .session_model(session_model)
        .build()
        .await;
    let host = &fixture.host;
    let (attachment, subscription) = attach(host);

    submit(&attachment, 1, "first");
    receive_until(&subscription, settled).await;
    await_request_history_len(host, 1).await;

    submit(&attachment, 2, "second");
    // The attempt is provably inside its first model stream.
    let mut parked = adapter.parked();
    tokio::time::timeout(LIVENESS, parked.wait_for(|parked| *parked))
        .await
        .expect("the model must park")
        .expect("the park watch stays open");

    // The update linearizes strictly after the attempt's admission.
    host.model_set(SessionModelConfig::of(
        ModelRef::parse("fixture/model-b").expect("valid reference"),
    ))
    .expect("the update is valid");

    release.send_replace(true);
    receive_until(&subscription, settled).await;
    await_request_history_len(host, 3).await;

    let requests = adapter.requests();
    assert_eq!(requests.len(), 4, "att1 + parked request + summary + retry");
    assert_eq!(requests[0].model(), "model-a");
    assert_eq!(
        requests[1].model(),
        "model-a",
        "the parked request kept the admitted model"
    );
    assert_eq!(
        requests[2].model(),
        "model-a",
        "session mode: the summary model is the attempt's frozen primary"
    );
    assert!(
        requests[2].tools.is_empty(),
        "the summary request stays the canonical one-off shape"
    );
    assert_eq!(requests[2].continuation, None);
    assert_eq!(
        requests[3].model(),
        "model-a",
        "the overflow retry stays inside the frozen attempt snapshot"
    );

    // The update is live for future attempts only.
    let view = fixture.runtime.model_view();
    assert_eq!(
        view.configured,
        SessionModelConfig::of(ModelRef::parse("fixture/model-b").expect("valid reference"))
    );

    submit(&attachment, 3, "third");
    receive_until(&subscription, settled).await;
    await_request_history_len(host, 4).await;
    let requests = adapter.requests();
    assert_eq!(requests.len(), 5);
    assert_eq!(
        requests[4].model(),
        "model-b",
        "the next admitted attempt resolves the updated session model"
    );
}
