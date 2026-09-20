//! Issue #108 (FND-03) — the Agent Loop half of the durable publication
//! contract.
//!
//! These regressions drive the real Agent Loop against a scripted model and a
//! real durable store, and assert what the publication plane does with the
//! provider's output:
//!
//! ```text
//! provider deltas -> assembler -> bounded coalescer -> durable staging -> release
//! ```
//!
//! Everything time-dependent is decided by an explicitly installed
//! [`CoalescePolicy`] and a manually advanced
//! [`ManualMonotonicClock`](rustx::runtime::ManualMonotonicClock).
//! No sleep proves any invariant here: a byte threshold, a structural
//! boundary, an advanced fake clock, or the stream terminal is what makes a
//! flush happen.
//!
//! The durable-store half of the same contract — the `C => U => P`
//! implication, settlement exclusivity, crash-boundary classification, and
//! audit consolidation — lives in `tests/durable/publication.rs`.

use super::super::{common, support};

use std::sync::Arc;
use std::time::Duration;

use rustx::agent::{AgentCancellation, AgentExecution, AgentExecutionRequest};
use rustx::durable::ConversationStore;
use rustx::events::types::{AttemptOutcome, RuntimeEvent};
use rustx::message::content::TextBlock;
use rustx::message::types::{
    ContentBlockIndex, InboundKind, MessageBlock, UserContentBlock, UserMessageBlock, UserSource,
};
use rustx::model::event::ModelEvent;
use rustx::model::finish::ModelFinishReason;
use rustx::publication::{
    CoalescePolicy, PublicationAuditBlock, PublicationAuditKind, PublicationPayload,
};
use rustx::runtime::identity::{AgentId, AttemptId, ConversationId, MessageId, ToolCallId, ToolId};
use rustx::runtime::types::{CancellationReason, RuntimeClock};
use rustx::runtime::{ManualMonotonicClock, MonotonicClock};
use rustx::tools::executor::ToolRegistry;
use rustx::tools::types::{ToolCall, ToolCallStart};
use support::fake::{FakeModel, FakeStep, fake_model};

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

fn request(conversation: &str, model: &Arc<FakeModel>) -> AgentExecutionRequest {
    AgentExecutionRequest {
        agent_id: AgentId::new("agent-a"),
        conversation_id: ConversationId::new(conversation),
        attempt_id: AttemptId::new("attempt-1"),
        conversation: rustx::conversation::ConversationState::from_messages(vec![
            MessageBlock::User(UserMessageBlock {
                id: MessageId::new("msg-user-1"),
                content: vec![UserContentBlock::Text(TextBlock {
                    text: "go".to_owned(),
                })],
                source: UserSource::Human,
                kind: InboundKind::Message,
                timestamp: None,
            }),
        ])
        .expect("bootstrap conversation"),
        initial_turn_trigger: rustx::agent::InitialTurnTrigger::Continuation,
        model: support::attempt_model(model.clone(), "fake-model"),
    }
}

fn context_runtime(model: &Arc<FakeModel>) -> rustx::context::ContextRuntime {
    use rustx::context::{ContextRuntime, DefaultTokenEstimator, SessionContextPolicy};
    let estimator: Arc<dyn rustx::context::TokenEstimator> = Arc::new(DefaultTokenEstimator);
    let snapshot = support::attempt_model(model.clone(), "fake-model");
    ContextRuntime::for_attempt(
        SessionContextPolicy {
            reserve_tokens: 0,
            keep_recent_tokens: 0,
            summary_output_cap: None,
        },
        estimator,
        Some(rustx::context::AgentStatusEngine::default()),
        &snapshot,
        rustx::model::ModelTimeoutPolicy::default(),
        support::default_monotonic_clock(),
    )
    .expect("valid context runtime")
}

/// The outcome of one scripted publication run.
struct Run {
    audit: common::DurableExecutionAudit,
    clock: Arc<ManualMonotonicClock>,
    publication_opened: Vec<rustx::publication::PublicationStreamStart>,
    publication_trace: Vec<common::PublicationObservation>,
}

/// A policy that never flushes on bytes or latency by itself, so a test that
/// wants no incidental flush gets none.
fn quiet_policy() -> CoalescePolicy {
    CoalescePolicy {
        max_bytes: usize::MAX,
        max_latency_millis: u64::MAX,
    }
}

#[derive(Debug, Clone, Copy)]
struct FixedRecoveryClock;

impl RuntimeClock for FixedRecoveryClock {
    fn now(&self) -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::parse_from_rfc3339("2026-08-24T12:00:00Z")
            .expect("fixed recovery timestamp")
            .with_timezone(&chrono::Utc)
    }
}

/// Drives one attempt with an explicitly installed publication policy over a
/// caller-supplied durable store.
async fn run_with(
    conversation: &str,
    model: &Arc<FakeModel>,
    tools: ToolRegistry,
    policy: CoalescePolicy,
    store: Option<Arc<rustx::durable::SqliteConversationStore>>,
) -> Run {
    let erased: Option<Arc<dyn ConversationStore>> =
        store.map(|store| store as Arc<dyn ConversationStore>);
    let fixture = common::tool_runtime_with_store(conversation, erased);
    let tool_runtime: rustx::tools::runtime::ConversationToolRuntime = (*fixture).clone();
    let durable = tool_runtime.durable_store();
    let capability = common::capability_lease(tools, &tool_runtime).await;
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let publication = common::RecordingPublicationObserver::default();
    let clock = Arc::new(ManualMonotonicClock::new());
    let mut execution = AgentExecution::new(
        request(conversation, model),
        capability.into_lease(),
        &cancellation,
        support::default_execution_policy(),
        context_runtime(model),
        &tool_runtime,
        rustx::agent::AttemptLifecycle::inert(),
    )
    .expect("conversation identity matches the tool runtime");
    execution.install_publication_policy(policy, Arc::clone(&clock) as Arc<dyn MonotonicClock>);
    execution.observe(&publication);
    let result = execution.run().await;
    let publication_opened = publication.opened();
    let publication_trace = publication.trace();
    Run {
        audit: common::durable_agent_result_with_publication(
            result,
            durable.as_ref(),
            &publication,
        ),
        clock,
        publication_opened,
        publication_trace,
    }
}

async fn run(conversation: &str, model: &Arc<FakeModel>, policy: CoalescePolicy) -> Run {
    run_with(conversation, model, ToolRegistry::new(), policy, None).await
}

fn started() -> FakeStep {
    FakeStep::Emit(ModelEvent::Started)
}

fn text(chunk: &str) -> FakeStep {
    FakeStep::Emit(ModelEvent::TextDelta {
        block_index: ContentBlockIndex::new(0),
        text: chunk.to_owned(),
    })
}

fn done(reason: ModelFinishReason) -> FakeStep {
    FakeStep::Emit(ModelEvent::Completed {
        finish_reason: reason,
        usage: None,
    })
}

fn proposal_start(call: &str) -> FakeStep {
    FakeStep::Emit(ModelEvent::ToolCallStarted {
        block_index: ContentBlockIndex::new(1),
        call: ToolCallStart {
            id: ToolCallId::new(call),
            tool_id: ToolId::new("tool-alpha"),
            name: "alpha".to_owned(),
        },
    })
}

fn proposal_arguments(call: &str, fragment: &str) -> FakeStep {
    FakeStep::Emit(ModelEvent::ToolCallArgumentsDelta {
        block_index: ContentBlockIndex::new(1),
        call_id: ToolCallId::new(call),
        arguments_delta: fragment.to_owned(),
    })
}

fn proposal_complete(call: &str) -> FakeStep {
    FakeStep::Emit(ModelEvent::ToolCallCompleted {
        block_index: ContentBlockIndex::new(1),
        call: ToolCall {
            id: ToolCallId::new(call),
            tool_id: ToolId::new("tool-alpha"),
            name: "alpha".to_owned(),
            arguments: serde_json::json!({}),
        },
    })
}

// ---------------------------------------------------------------------------
// Bounded coalescing (regressions 1 and 2)
// ---------------------------------------------------------------------------

/// **Regression 1.** Many provider deltas coalesce into far fewer durable
/// publication writes under a byte threshold. Provider chunk size is not the
/// publication unit, and no released byte is lost by the coalescing.
#[tokio::test]
async fn many_provider_deltas_coalesce_into_few_publication_writes() {
    // 40 single-character deltas at a 10-byte threshold.
    let mut script = vec![started()];
    for index in 0..40u32 {
        script.push(text(&(index % 10).to_string()));
    }
    script.push(done(ModelFinishReason::Stop));
    let model = fake_model(vec![script]);
    let run = run(
        "conv_52da5d5b-4895-7cc9-a592-012b9626a213",
        &model,
        CoalescePolicy {
            max_bytes: 10,
            max_latency_millis: u64::MAX,
        },
    )
    .await;

    assert_eq!(
        run.audit.publication_frames.len(),
        5,
        "40 provider deltas became 4 threshold flushes plus the terminal frame"
    );
    assert_eq!(
        run.audit.released_publication_text().len(),
        40,
        "coalescing loses no released byte"
    );
    // The frame sequence is deterministic and gapless.
    let sequences: Vec<u64> = run
        .audit
        .publication_frames
        .iter()
        .map(|frame| frame.sequence)
        .collect();
    assert_eq!(sequences, vec![0, 1, 2, 3, 4]);
    assert!(run.audit.publication_audits.is_empty());
}

/// **Regression 2.** The latency flush is decided by the injected clock
/// alone. The same script under a never-elapsing clock produces one frame;
/// advancing the fake clock past the threshold produces one frame per delta.
#[tokio::test]
async fn latency_flush_is_driven_by_the_injected_clock() {
    let script = || {
        vec![vec![
            started(),
            text("alpha"),
            text("beta"),
            text("gamma"),
            done(ModelFinishReason::Stop),
        ]]
    };

    // The clock never advances, so only the terminal transaction flushes.
    let parked = run(
        "conv_3ca052fa-fbf2-72af-8f93-5d4c315a82b1",
        &fake_model(script()),
        quiet_policy(),
    )
    .await;
    assert_eq!(
        parked.audit.publication_frames.len(),
        1,
        "with no byte or latency trigger, only the terminal transaction flushes"
    );
    assert_eq!(parked.audit.released_publication_text(), "alphabetagamma");

    // The same script under a clock that always reports the threshold as
    // elapsed flushes on every delta instead.
    let flushing = run(
        "conv_d764b1ab-7946-7581-b9bd-fdd59d5af45c",
        &fake_model(script()),
        CoalescePolicy {
            max_bytes: usize::MAX,
            max_latency_millis: 0,
        },
    )
    .await;
    assert_eq!(
        flushing.audit.publication_frames.len(),
        4,
        "an already-elapsed latency window flushes each delta, plus the terminal frame"
    );
    assert_eq!(flushing.audit.released_publication_text(), "alphabetagamma");
    // The manual clock proves nothing waited on wall-clock time.
    assert_eq!(flushing.clock.now_millis(), 0);
}

/// The Agent Loop uses the oldest buffered payload's deadline rather than a
/// fresh full-duration sleep. A second delta arriving at t=49 ms cannot move
/// the first release past the original t=50 ms boundary.
#[tokio::test]
async fn chatty_provider_cannot_postpone_the_oldest_publication_deadline() {
    let (first_release, first_receiver) = support::fake::model_release();
    let (second_release, second_receiver) = support::fake::model_release();
    let model = fake_model(vec![vec![
        started(),
        text("a"),
        FakeStep::ParkUntilReleased(first_receiver),
        text("b"),
        FakeStep::ParkUntilReleased(second_receiver),
        done(ModelFinishReason::Stop),
    ]]);
    let fixture =
        common::tool_runtime_with_store("conv_5088f8c6-d684-7d86-8008-bcdd7b9ac230", None);
    let tool_runtime: rustx::tools::runtime::ConversationToolRuntime = (*fixture).clone();
    let capability = common::capability_lease(ToolRegistry::new(), &tool_runtime).await;
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let publication = common::RecordingPublicationObserver::default();
    let clock = Arc::new(ManualMonotonicClock::new());
    let mut execution = AgentExecution::new(
        request("conv_5088f8c6-d684-7d86-8008-bcdd7b9ac230", &model),
        capability.into_lease(),
        &cancellation,
        support::default_execution_policy(),
        context_runtime(&model),
        &tool_runtime,
        rustx::agent::AttemptLifecycle::inert(),
    )
    .expect("conversation identity matches the tool runtime");
    execution.install_publication_policy(
        CoalescePolicy {
            max_bytes: usize::MAX,
            max_latency_millis: 50,
        },
        Arc::clone(&clock) as Arc<dyn MonotonicClock>,
    );
    execution.observe(&publication);
    let execution = execution.run();
    tokio::pin!(execution);

    let mut emitted = model.emitted();
    let mut parked = model.parked();
    tokio::select! {
        result = &mut execution => panic!("agent execution ended before first delta: {result:?}"),
        count = emitted.wait_for(|count| *count >= 2) => {
            count.expect("first delta emitted");
        }
    }
    tokio::select! {
        result = &mut execution => panic!("agent execution ended before provider park: {result:?}"),
        is_parked = parked.wait_for(|is_parked| *is_parked) => {
            is_parked.expect("provider parked after first delta");
        }
    }
    let mut frame_count = publication.frame_count();

    clock.advance(49);
    assert_eq!(*frame_count.borrow(), 0, "no early release before t=50 ms");
    first_release.send_replace(true);
    tokio::select! {
        result = &mut execution => panic!("agent execution ended before second delta: {result:?}"),
        count = emitted.wait_for(|count| *count >= 3) => {
            count.expect("second delta emitted at t=49 ms");
        }
    }

    // If the loop restarted a 50 ms timer here, this liveness guard would
    // expire. It is only a guard; the exact boundary is controlled by the
    // manual publication clock.
    clock.advance(1);
    tokio::time::timeout(Duration::from_secs(10), async {
        tokio::select! {
            result = &mut execution => panic!("agent execution ended before deadline release: {result:?}"),
            count = frame_count.wait_for(|count| *count >= 1) => {
                count.expect("publication observer remains connected");
            }
        }
    })
    .await
    .expect("oldest publication deadline wakes the loop");
    assert_eq!(publication.released_text(), "ab");

    second_release.send_replace(true);
    let _ = execution.await;
}

/// A quiet provider still wakes the Agent Loop at the coalescer deadline;
/// another provider event is not required to trigger the latency flush.
#[tokio::test]
async fn quiet_provider_is_woken_by_the_publication_deadline() {
    let (release, receiver) = support::fake::model_release();
    let model = fake_model(vec![vec![
        started(),
        text("quiet"),
        FakeStep::ParkUntilReleased(receiver),
        done(ModelFinishReason::Stop),
    ]]);
    let fixture =
        common::tool_runtime_with_store("conv_99c4255f-a23f-7e98-aad5-c738b458e2a5", None);
    let tool_runtime: rustx::tools::runtime::ConversationToolRuntime = (*fixture).clone();
    let capability = common::capability_lease(ToolRegistry::new(), &tool_runtime).await;
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let publication = common::RecordingPublicationObserver::default();
    let clock = Arc::new(ManualMonotonicClock::new());
    let mut execution = AgentExecution::new(
        request("conv_99c4255f-a23f-7e98-aad5-c738b458e2a5", &model),
        capability.into_lease(),
        &cancellation,
        support::default_execution_policy(),
        context_runtime(&model),
        &tool_runtime,
        rustx::agent::AttemptLifecycle::inert(),
    )
    .expect("conversation identity matches the tool runtime");
    execution.install_publication_policy(
        CoalescePolicy {
            max_bytes: usize::MAX,
            max_latency_millis: 50,
        },
        Arc::clone(&clock) as Arc<dyn MonotonicClock>,
    );
    execution.observe(&publication);
    let execution = execution.run();
    tokio::pin!(execution);

    let mut emitted = model.emitted();
    let mut parked = model.parked();
    tokio::select! {
        result = &mut execution => panic!("agent execution ended before quiet payload: {result:?}"),
        count = emitted.wait_for(|count| *count >= 2) => {
            count.expect("quiet provider emitted its payload");
        }
    }
    tokio::select! {
        result = &mut execution => panic!("agent execution ended before quiet provider park: {result:?}"),
        is_parked = parked.wait_for(|is_parked| *is_parked) => {
            is_parked.expect("quiet provider is parked");
        }
    }
    let mut frame_count = publication.frame_count();
    assert_eq!(
        *frame_count.borrow(),
        0,
        "payload remains buffered before deadline"
    );

    clock.advance(50);
    tokio::time::timeout(Duration::from_secs(10), async {
        tokio::select! {
            result = &mut execution => panic!("agent execution ended before quiet deadline release: {result:?}"),
            count = frame_count.wait_for(|count| *count >= 1) => {
                count.expect("publication observer remains connected");
            }
        }
    })
    .await
    .expect("quiet provider deadline wakes the loop");
    assert_eq!(publication.released_text(), "quiet");

    release.send_replace(true);
    let _ = execution.await;
    assert_eq!(
        model.requests().len(),
        1,
        "the test has one provider request"
    );
}

/// A tool-call proposal start and completion are structural boundaries: each
/// is released as its own frame rather than being coalesced into the
/// surrounding text.
#[tokio::test]
async fn tool_proposal_boundaries_are_released_as_their_own_frames() {
    let model = fake_model(vec![vec![
        started(),
        text("thinking"),
        proposal_start("call-1"),
        proposal_arguments("call-1", "{}"),
        proposal_complete("call-1"),
        done(ModelFinishReason::ToolCalls),
    ]]);
    let definition = common::tool_policies(
        "alpha",
        "tool-alpha",
        rustx::tools::types::ToolExecutionPolicy::ForegroundOnly,
        rustx::tools::types::ToolConcurrencyPolicy::Sequential,
    );
    let mut tools = ToolRegistry::new();
    support::fake::FakeTool::new(definition, support::fake::success_result("ok"))
        .register(&mut tools);
    let run = run_with(
        "conv_6a0e034d-cbae-7d44-91e1-1601d8effa70",
        &model,
        tools,
        quiet_policy(),
        None,
    )
    .await;

    let payloads: Vec<&PublicationPayload> = run
        .audit
        .publication_frames
        .iter()
        .map(|frame| &frame.payload)
        .collect();
    assert!(matches!(payloads[0], PublicationPayload::TextSuffix { .. }));
    assert!(matches!(
        payloads[1],
        PublicationPayload::ProposedToolCallStarted { .. }
    ));
    assert!(
        payloads.iter().any(|payload| matches!(
            payload,
            PublicationPayload::ProposedToolCallCompleted { .. }
        )),
        "the completed proposal is its own observable transition"
    );
}

// ---------------------------------------------------------------------------
// Release ordering (regression 3)
// ---------------------------------------------------------------------------

/// **Regression 3.** No frame is released before its staging commit. When the
/// staging transaction fails, nothing reaches the observation seam and the
/// attempt reports a publication durability failure rather than pretending
/// the output was published.
#[tokio::test]
async fn no_frame_is_released_before_its_staging_commit() {
    let store = Arc::new(
        rustx::durable::SqliteConversationStore::in_memory(ConversationId::new(
            "conv_f927f4f5-3c30-76ad-855f-361912246400",
        ))
        .expect("durable store"),
    );
    store.arm_fail_publication_frames_times(1);
    let model = fake_model(vec![vec![
        started(),
        text("never released"),
        done(ModelFinishReason::Stop),
    ]]);
    let run = run_with(
        "conv_f927f4f5-3c30-76ad-855f-361912246400",
        &model,
        ToolRegistry::new(),
        CoalescePolicy {
            max_bytes: 4,
            max_latency_millis: u64::MAX,
        },
        Some(Arc::clone(&store)),
    )
    .await;

    assert!(
        run.audit.publication_frames.is_empty(),
        "a frame whose staging transaction failed is never released"
    );
    assert_eq!(
        run.audit
            .durable_failure_kind
            .map(rustx::agent::DurableFailureKind::as_str),
        Some("publication"),
        "the attempt reports the publication-plane durability failure"
    );
    assert!(matches!(run.audit.outcome, AttemptOutcome::Failed { .. }));
    assert!(
        run.audit.messages().len() == 1,
        "no Assistant message became canonical"
    );
}

/// A failing publication terminal transaction (U) fails the attempt and
/// releases no final payload: the buffered tail waits for a commit that never
/// happened.
#[tokio::test]
async fn a_failed_publication_terminal_releases_no_final_payload() {
    let store = Arc::new(
        rustx::durable::SqliteConversationStore::in_memory(ConversationId::new(
            "conv_773452a9-b122-7082-8fb0-4a674050176c",
        ))
        .expect("durable store"),
    );
    store.arm_fail_publication_terminal_times(1);
    let model = fake_model(vec![vec![
        started(),
        text("tail"),
        done(ModelFinishReason::Stop),
    ]]);
    let run = run_with(
        "conv_773452a9-b122-7082-8fb0-4a674050176c",
        &model,
        ToolRegistry::new(),
        quiet_policy(),
        Some(Arc::clone(&store)),
    )
    .await;

    assert!(
        run.audit.publication_frames.is_empty(),
        "the buffered tail is released only after U commits"
    );
    assert!(matches!(run.audit.outcome, AttemptOutcome::Failed { .. }));
    // P committed before U was attempted: the provider outcome remains an
    // external execution fact even though publication never settled.
    assert!(
        run.audit
            .event_history
            .iter()
            .any(|event| matches!(event, RuntimeEvent::ModelRequestCompleted { .. })),
        "P is durable and independent of the failed U"
    );
    let audit = run
        .audit
        .publication_audits
        .first()
        .expect("the stream settled as an audit");
    assert_eq!(
        audit.kind,
        PublicationAuditKind::Incomplete,
        "P present but U absent is Incomplete, not Unaccepted"
    );
}

// ---------------------------------------------------------------------------
// Settlement per control-flow exit (regressions 9, 12, 14)
// ---------------------------------------------------------------------------

/// **Regression 9.** A structural `assembler.finish()` rejection after frames
/// were already published terminalizes as **Incomplete**, and no provider
/// outcome (P) is committed at all — the physically ended stream was never
/// structurally accepted.
#[tokio::test]
async fn assembler_finish_failure_after_published_frames_is_incomplete() {
    // The proposal starts and the stream terminates without completing it:
    // `assembler.finish()` rejects the turn.
    let model = fake_model(vec![vec![
        started(),
        text("visible"),
        proposal_start("call-1"),
        done(ModelFinishReason::ToolCalls),
    ]]);
    let run = run(
        "conv_28323f58-01b4-7ec8-82ba-e5156e469e84",
        &model,
        quiet_policy(),
    )
    .await;

    assert!(
        !run.audit.publication_frames.is_empty(),
        "the proposal start was a structural boundary and was already released"
    );
    assert!(
        !run.audit
            .event_history
            .iter()
            .any(|event| matches!(event, RuntimeEvent::ModelRequestCompleted { .. })),
        "a structurally rejected stream commits no provider outcome"
    );
    let audit = run
        .audit
        .publication_audits
        .first()
        .expect("the stream settled as an audit");
    assert_eq!(
        audit.kind,
        PublicationAuditKind::Incomplete,
        "published frames plus a structural rejection is Incomplete, never Unaccepted"
    );
    assert!(
        audit.content.iter().any(|block| matches!(
            block,
            PublicationAuditBlock::ProposedToolCall { complete, .. } if !complete
        )),
        "the audit records the released proposal as incomplete"
    );
    assert_eq!(
        run.audit.messages().len(),
        1,
        "no Assistant message became canonical"
    );
}

/// **Regression 12.** A complete model-proposed tool call whose preflight
/// contract fails settles as an **Unaccepted** proposal audit: no canonical
/// Assistant message, no `ToolExecutionStarted`, and no `ToolResult`.
#[tokio::test]
async fn preflight_failure_after_a_complete_proposal_is_unaccepted() {
    // The tool registry is empty, so preflight rejects the proposed call as an
    // unregistered tool after the model output is structurally complete.
    let model = fake_model(vec![vec![
        started(),
        text("calling"),
        proposal_start("call-1"),
        proposal_arguments("call-1", "{}"),
        proposal_complete("call-1"),
        done(ModelFinishReason::ToolCalls),
    ]]);
    let run = run(
        "conv_57b70711-c789-73c2-a31e-df12a1c97700",
        &model,
        quiet_policy(),
    )
    .await;

    assert!(
        run.audit
            .event_history
            .iter()
            .any(|event| matches!(event, RuntimeEvent::ModelRequestCompleted { .. })),
        "the provider outcome (P) committed before the preflight boundary"
    );
    let audit = run
        .audit
        .publication_audits
        .first()
        .expect("the stream settled as an audit");
    assert_eq!(
        audit.kind,
        PublicationAuditKind::Unaccepted,
        "U reached, C never: the output was complete but was never accepted"
    );
    assert!(
        audit.content.iter().any(|block| matches!(
            block,
            PublicationAuditBlock::ProposedToolCall { complete, .. } if *complete
        )),
        "the audit records a complete model proposal"
    );

    assert_eq!(
        run.audit.messages().len(),
        1,
        "no Assistant message became canonical"
    );
    assert!(
        !run.audit.event_history.iter().any(|event| matches!(
            event,
            RuntimeEvent::ToolExecutionStarted { .. } | RuntimeEvent::ToolMessageCommitted { .. }
        )),
        "an unaccepted proposal has no dependent Tool Plane execution fact"
    );
    assert!(matches!(run.audit.outcome, AttemptOutcome::Failed { .. }));
}

/// **Regression 14.** A canonically accepted turn creates no publication
/// audit at all, and the durable plane retains no staging for it.
#[tokio::test]
async fn a_canonically_accepted_turn_creates_no_audit_and_no_staging() {
    let store = Arc::new(
        rustx::durable::SqliteConversationStore::in_memory(ConversationId::new(
            "conv_5fae99b2-17ed-70c2-8c0a-d9455f2477d8",
        ))
        .expect("durable store"),
    );
    let model = fake_model(vec![vec![
        started(),
        text("hello "),
        text("world"),
        done(ModelFinishReason::Stop),
    ]]);
    let run = run_with(
        "conv_5fae99b2-17ed-70c2-8c0a-d9455f2477d8",
        &model,
        ToolRegistry::new(),
        quiet_policy(),
        Some(Arc::clone(&store)),
    )
    .await;

    assert!(matches!(
        run.audit.outcome,
        AttemptOutcome::Completed { .. }
    ));
    assert!(
        run.audit.publication_audits.is_empty(),
        "canonical acceptance and audit terminalization are mutually exclusive"
    );
    assert!(
        store
            .load_unsettled_publication_streams()
            .expect("unsettled")
            .is_empty(),
        "the canonical transition cleared the publication staging"
    );
    let MessageBlock::Assistant(assistant) =
        run.audit.messages().last().expect("Assistant message")
    else {
        panic!("the final message is the canonical Assistant message");
    };
    let rustx::message::types::AssistantContentBlock::Text(block) = &assistant.content[0] else {
        panic!("the canonical message carries the assembled text");
    };
    assert_eq!(block.text, "hello world");
    assert_eq!(
        run.audit.released_publication_text(),
        "hello world",
        "what was released for display equals what became canonical"
    );
}

/// A failing audit terminalization leaves the stream durably unsettled rather
/// than pretending it settled, so the next startup recovery still classifies
/// it truthfully. Every committed prefix of settlement is a valid input to a
/// later recovery.
#[tokio::test]
async fn a_failed_audit_terminalization_leaves_the_stream_unsettled() {
    let store = Arc::new(
        rustx::durable::SqliteConversationStore::in_memory(ConversationId::new(
            "conv_6d41dbd2-3d82-7b3c-8f38-47d602ea7b7e",
        ))
        .expect("durable store"),
    );
    store.arm_fail_publication_audit_times(1);
    // A proposal that never completes rejects `assembler.finish()`, so the
    // turn exits through the audit path.
    let model = fake_model(vec![vec![
        started(),
        text("visible"),
        proposal_start("call-1"),
        done(ModelFinishReason::ToolCalls),
    ]]);
    let run = run_with(
        "conv_6d41dbd2-3d82-7b3c-8f38-47d602ea7b7e",
        &model,
        ToolRegistry::new(),
        quiet_policy(),
        Some(Arc::clone(&store)),
    )
    .await;

    assert!(
        run.audit.publication_audits.is_empty(),
        "no audit is observed when its transaction failed"
    );
    assert_eq!(
        run.audit
            .durable_failure_kind
            .map(rustx::agent::DurableFailureKind::as_str),
        Some("publication"),
        "the failed terminalization is reported as a publication durability failure"
    );
    let unsettled = store
        .load_unsettled_publication_streams()
        .expect("unsettled");
    assert_eq!(unsettled.len(), 1, "the stream is still recoverable");
    assert_eq!(
        unsettled[0].audit_kind(),
        PublicationAuditKind::Incomplete,
        "a later recovery reaches the same classification from the same evidence"
    );
}

/// Overflow retry preparation is allowed to begin only after the abandoned
/// publication's audit is durable. If that settlement fails, the attempt
/// stops at the original stream: no second Request Snapshot, provider start,
/// adapter invocation, or publication open may exist.
#[tokio::test]
async fn failed_overflow_audit_blocks_the_retry_request() {
    let store = Arc::new(
        rustx::durable::SqliteConversationStore::in_memory(ConversationId::new(
            "conv_487d78b2-8e80-7af8-85a4-30ac967fb263",
        ))
        .expect("durable store"),
    );
    store.arm_fail_publication_audit_times(1);
    let model = fake_model(vec![
        vec![
            started(),
            FakeStep::Emit(ModelEvent::Failed {
                error: rustx::model::ModelError {
                    kind: rustx::model::ModelErrorKind::ContextWindowExceeded,
                    message: "context window exceeded".to_owned(),
                    retry_disposition: rustx::model::error::ModelRetryDisposition::Never,
                    retry_after_ms: None,
                    provider_code: None,
                    context_overflow: None,
                    malformed_tool_proposal: None,
                    timeout_phase: None,
                    generation: None,
                },
            }),
        ],
        vec![started(), done(ModelFinishReason::Stop)],
    ]);
    let run = run_with(
        "conv_487d78b2-8e80-7af8-85a4-30ac967fb263",
        &model,
        ToolRegistry::new(),
        quiet_policy(),
        Some(Arc::clone(&store)),
    )
    .await;

    assert_eq!(
        model.requests().len(),
        1,
        "the retry adapter is never invoked after audit failure"
    );
    assert_eq!(run.audit.snapshot_history().len(), 1);
    assert_eq!(
        run.audit
            .event_history
            .iter()
            .filter(|event| matches!(event, RuntimeEvent::ModelRequestStarted { .. }))
            .count(),
        1,
        "only the original Request Snapshot reached ModelRequestStarted"
    );
    assert!(
        !run.audit
            .event_history
            .iter()
            .any(|event| matches!(event, RuntimeEvent::ModelRetryScheduled { .. }))
    );
    assert_eq!(run.publication_opened.len(), 1);
    assert!(matches!(
        run.publication_trace.as_slice(),
        [common::PublicationObservation::Opened(_)]
    ));
    assert_eq!(
        run.audit
            .durable_failure_kind
            .map(rustx::agent::DurableFailureKind::as_str),
        Some("publication")
    );
    assert!(matches!(run.audit.outcome, AttemptOutcome::Failed { .. }));

    let unsettled = store
        .load_unsettled_publication_streams()
        .expect("unsettled stream");
    assert_eq!(unsettled.len(), 1);
    assert!(
        store
            .load_publication_audit(&run.publication_opened[0].stream_id)
            .expect("audit")
            .is_none()
    );

    let report = rustx::runtime::recovery::recover(&*store, &FixedRecoveryClock)
        .expect("startup recovery classifies the original stream");
    assert_eq!(report.publication_classes().len(), 1);
    assert_eq!(
        report.publication_classes()[0].kind,
        PublicationAuditKind::Incomplete
    );
}

// ---------------------------------------------------------------------------
// Event Journal write amplification (regression 16)
// ---------------------------------------------------------------------------

/// **Regression 16.** The Event Journal does not grow per Assistant streaming
/// increment: a 200-delta turn journals exactly what a 2-delta turn does.
#[tokio::test]
async fn the_event_journal_is_independent_of_the_delta_count() {
    async fn journal_len(conversation: &str, deltas: usize) -> usize {
        let mut script = vec![started()];
        for _ in 0..deltas {
            script.push(text("x"));
        }
        script.push(done(ModelFinishReason::Stop));
        let model = fake_model(vec![script]);
        let run = run(
            conversation,
            &model,
            CoalescePolicy {
                max_bytes: 1,
                max_latency_millis: u64::MAX,
            },
        )
        .await;
        // Every delta flushed under the one-byte threshold, so the publication
        // plane really did do per-delta work.
        assert!(run.audit.publication_frames.len() >= deltas);
        run.audit.event_history.len()
    }

    let quiet = journal_len("conv_bf971fb7-a072-7c1a-bb8c-0337887594bc", 2).await;
    let chatty = journal_len("conv_28351899-1298-7aa1-8b2d-ed78ff260826", 200).await;
    assert_eq!(
        quiet, chatty,
        "198 additional released increments cost zero additional Event Journal rows"
    );
}

// ---------------------------------------------------------------------------
// Request-pinned resource generation (regressions 17 and 18)
// ---------------------------------------------------------------------------
