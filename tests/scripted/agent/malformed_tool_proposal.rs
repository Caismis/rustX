//! Issue #201: the malformed-tool-proposal boundary in the Agent Loop.
//!
//! A physical generation whose tool intent could not cross `ToolCall`
//! acceptance is **noncanonical**. This suite owns the Agent-Loop half of
//! that contract: the discarded generation executes nothing and commits
//! nothing, the same logical step is regenerated exactly once, a second
//! malformed generation terminates explicitly, and the malformed output
//! never appears in canonical history or in a reconstructed request.
//!
//! It also owns the *lifetime* half: the bounded corrective hint belongs to
//! the corrective **generation**, so it survives every actual request that
//! generation needs — including a transient transport retry and a
//! context-overflow recovery — and disappears the moment that generation
//! resolves.
//!
//! The provider half — which provider evidence becomes which
//! [`MalformedToolProposalSource`], including the Qwen reserved-markup class
//! and the guard against a naive substring rule — is owned by
//! `tests/provider/openai_chat.rs` and `tests/provider/anthropic.rs`. This
//! suite deliberately drives the *provider-independent* semantic outcome, so
//! nothing here knows a provider protocol.
//!
//! All synchronization is scripted; no test depends on elapsed time.

use super::super::{common, support};

use std::sync::Arc;

use rustx::agent::{AgentCancellation, AgentExecution, AgentExecutionRequest};
use rustx::context::{
    AgentStatusEngine, CompactionBudgets, ContextConfig, ContextEngine, ContextRuntime,
};
use rustx::events::types::{AttemptFailure, AttemptOutcome, RuntimeEvent};
use rustx::message::content::TextBlock;
use rustx::message::types::{
    AssistantContentBlock, ContentBlockIndex, MessageBlock, ToolMessageBlock, UserContentBlock,
    UserMessageBlock, UserSource,
};
use rustx::model::error::ContextOverflowReport;
use rustx::model::event::ModelEvent;
use rustx::model::finish::ModelFinishReason;
use rustx::model::{
    MalformedToolProposalSource, ModelError, ModelErrorKind, ModelInputMessage,
    ModelRetryDisposition,
};
use rustx::runtime::identity::{AgentId, AttemptId, ConversationId, MessageId};
use rustx::runtime::types::CancellationReason;
use rustx::tools::executor::ToolRegistry;
use rustx::tools::types::{ToolConcurrencyPolicy, ToolExecutionPolicy, ToolExecutionStatus};
use support::audit::{assert_outcome, assert_single_terminal};
use support::context::{FakeContextSummarizer, FakeSummaryStep, ScriptedEstimator};
use support::fake::{
    FakeModel, FakeStep, FakeTool, ScriptedCall, fake_model, success_result, tool_call_events,
};

const CONVERSATION: &str = "conv-201";
const MALFORMED_MARKER: &str = "no usable invocation id";

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

fn request(model: &Arc<FakeModel>) -> AgentExecutionRequest {
    AgentExecutionRequest {
        agent_id: AgentId::new("agent-201"),
        conversation_id: ConversationId::new(CONVERSATION),
        attempt_id: AttemptId::new("attempt-201"),
        conversation: rustx::conversation::ConversationState::from_messages(vec![
            MessageBlock::User(UserMessageBlock {
                id: MessageId::new("msg-user-1"),
                content: vec![UserContentBlock::Text(TextBlock {
                    text: "write the note".to_owned(),
                })],
                source: UserSource::Human,
                kind: rustx::message::types::InboundKind::Message,
                timestamp: None,
            }),
        ])
        .expect("bootstrap conversation"),
        initial_turn_trigger: rustx::agent::InitialTurnTrigger::Continuation,
        model: support::attempt_model(model.clone(), "fake-model"),
    }
}

fn runtime(model: &Arc<FakeModel>) -> rustx::context::ContextRuntime {
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
        rustx::context::AgentStatusEngine::default(),
        &snapshot,
        rustx::model::ModelTimeoutPolicy::default(),
        support::default_monotonic_clock(),
    )
    .expect("valid context runtime")
}

/// A context runtime whose window is small enough that one compaction
/// actually runs, so the overflow recovery path can be driven end to end.
fn compacting_runtime() -> ContextRuntime {
    let engine = ContextEngine::new(
        ContextConfig {
            context_window_tokens: 500,
            reserve_tokens: 0,
            keep_recent_tokens: 5,
        },
        Arc::new(ScriptedEstimator::new(100, 10, 0)),
    )
    .expect("valid context configuration");
    ContextRuntime::with_scripted_summarizer(
        engine,
        Arc::new(FakeContextSummarizer::new(vec![FakeSummaryStep::Return(
            "summary".to_owned(),
        )])),
        AgentStatusEngine::default(),
        CompactionBudgets::new(1, 1, 1_000_000),
    )
}

async fn run_with_runtime(
    model: &Arc<FakeModel>,
    tools: ToolRegistry,
    cancellation: &AgentCancellation,
    context_runtime: ContextRuntime,
) -> common::DurableExecutionAudit {
    let tool_runtime = common::tool_runtime(CONVERSATION);
    let store = tool_runtime.durable_store();
    let capability = common::capability_lease(tools, &tool_runtime).await;
    let result = AgentExecution::new(
        request(model),
        capability.into_lease(),
        cancellation,
        support::default_execution_policy(),
        context_runtime,
        &tool_runtime,
        rustx::agent::AttemptLifecycle::inert(),
    )
    .expect("conversation identity matches the tool runtime")
    .run()
    .await;
    common::durable_agent_result(result, store.as_ref())
}

async fn run(
    model: &Arc<FakeModel>,
    tools: ToolRegistry,
    cancellation: &AgentCancellation,
) -> common::DurableExecutionAudit {
    let tool_runtime = common::tool_runtime(CONVERSATION);
    let store = tool_runtime.durable_store();
    let capability = common::capability_lease(tools, &tool_runtime).await;
    let result = AgentExecution::new(
        request(model),
        capability.into_lease(),
        cancellation,
        support::default_execution_policy(),
        runtime(model),
        &tool_runtime,
        rustx::agent::AttemptLifecycle::inert(),
    )
    .expect("conversation identity matches the tool runtime")
    .run()
    .await;
    common::durable_agent_result(result, store.as_ref())
}

/// One refused tool proposal, exactly as an adapter reports it: a normalized
/// model failure carrying the provider-independent class and provenance.
fn malformed(source: MalformedToolProposalSource, message: &str) -> ModelEvent {
    ModelEvent::Failed {
        error: ModelError::malformed_tool_proposal(source, message.to_owned()),
    }
}

fn transient(message: &str) -> ModelEvent {
    ModelEvent::Failed {
        error: ModelError {
            kind: ModelErrorKind::RateLimit,
            message: message.to_owned(),
            retry_disposition: ModelRetryDisposition::Transient,
            // Zero delay keeps the transient path deterministic without a
            // manual clock; the backoff schedule itself is owned by
            // `scripted_suites::agent::retry`.
            retry_after_ms: Some(0),
            provider_code: Some("rate_limit_error".to_owned()),
            context_overflow: None,
            malformed_tool_proposal: None,
            timeout_phase: None,
            generation: None,
        },
    }
}

/// A provider rejection of the request as too large for the context window.
/// It is a rejected *actual request*, not a model generation, so it recovers
/// through compaction inside whatever generation it interrupted.
fn overflow(message: &str) -> ModelEvent {
    ModelEvent::Failed {
        error: ModelError {
            kind: ModelErrorKind::ContextWindowExceeded,
            message: message.to_owned(),
            retry_disposition: ModelRetryDisposition::Never,
            retry_after_ms: None,
            provider_code: Some("context_length_exceeded".to_owned()),
            context_overflow: Some(ContextOverflowReport {
                reported_input_tokens: None,
                context_limit: None,
            }),
            malformed_tool_proposal: None,
            timeout_phase: None,
            generation: None,
        },
    }
}

/// The physical generation that leaked reserved markup instead of emitting a
/// structured call: provisional assistant text, then the refusal. The text is
/// what must never reach canonical history.
fn leaking_generation() -> Vec<FakeStep> {
    vec![
        FakeStep::Emit(ModelEvent::Started),
        FakeStep::Emit(ModelEvent::TextDelta {
            block_index: ContentBlockIndex::new(0),
            text: "<parameter=path>\nnotes.txt\n</parameter>".to_owned(),
        }),
        FakeStep::Emit(malformed(
            MalformedToolProposalSource::ReservedProtocolLeak,
            "the model leaked the reserved Qwen XML tool-protocol envelope \
             <parameter=…</parameter> into ordinary output and produced no structured tool call",
        )),
    ]
}

fn refused_generation(source: MalformedToolProposalSource) -> Vec<FakeStep> {
    vec![
        FakeStep::Emit(ModelEvent::Started),
        FakeStep::Emit(malformed(source, MALFORMED_MARKER)),
    ]
}

fn stop_generation(text: &str) -> Vec<FakeStep> {
    vec![
        FakeStep::Emit(ModelEvent::Started),
        FakeStep::Emit(ModelEvent::TextDelta {
            block_index: ContentBlockIndex::new(0),
            text: text.to_owned(),
        }),
        FakeStep::Emit(ModelEvent::Completed {
            finish_reason: ModelFinishReason::Stop,
            usage: None,
        }),
    ]
}

fn tool_generation(call: &ScriptedCall) -> Vec<FakeStep> {
    let mut steps = vec![FakeStep::Emit(ModelEvent::Started)];
    for event in tool_call_events(0, call) {
        steps.push(FakeStep::Emit(event));
    }
    steps.push(FakeStep::Emit(ModelEvent::Completed {
        finish_reason: ModelFinishReason::ToolCalls,
        usage: None,
    }));
    steps
}

fn write_call(arguments: serde_json::Value) -> ScriptedCall {
    ScriptedCall {
        id: "call-201",
        tool_id: "tool-write",
        name: "write_file",
        arguments,
    }
}

fn write_tool_definition() -> rustx::tools::types::ToolDefinition {
    common::tool_policies(
        "write_file",
        "tool-write",
        ToolExecutionPolicy::ForegroundOnly,
        ToolConcurrencyPolicy::Sequential,
    )
}

// ---------------------------------------------------------------------------
// Assertions
// ---------------------------------------------------------------------------

fn assistant_texts(audit: &common::DurableExecutionAudit) -> Vec<String> {
    audit
        .messages()
        .iter()
        .filter_map(|message| match message {
            MessageBlock::Assistant(assistant) => Some(
                assistant
                    .content
                    .iter()
                    .filter_map(|block| match block {
                        AssistantContentBlock::Text(text) => Some(text.text.clone()),
                        _ => None,
                    })
                    .collect::<String>(),
            ),
            _ => None,
        })
        .collect()
}

fn tool_messages(audit: &common::DurableExecutionAudit) -> Vec<&ToolMessageBlock> {
    audit
        .messages()
        .iter()
        .filter_map(|message| match message {
            MessageBlock::Tool(tool) => Some(tool),
            _ => None,
        })
        .collect()
}

fn retry_schedules(events: &[RuntimeEvent]) -> usize {
    events
        .iter()
        .filter(|event| matches!(event, RuntimeEvent::ModelRetryScheduled { .. }))
        .count()
}

/// Every canonical history text of the attempt, as one haystack.
fn canonical_text(audit: &common::DurableExecutionAudit) -> String {
    audit
        .messages()
        .iter()
        .map(|message| serde_json::to_string(message).expect("canonical message serializes"))
        .collect()
}

/// Every message the model was ever asked to reason over, across all
/// physical requests, as one haystack. This is the reconstruction projection
/// a later turn actually receives.
fn reconstructed_request_text(model: &Arc<FakeModel>) -> String {
    model
        .requests()
        .iter()
        .flat_map(|request| request.messages.iter())
        .map(|message| match message {
            ModelInputMessage::Canonical(block) => {
                serde_json::to_string(block).expect("canonical message serializes")
            }
            ModelInputMessage::RequestOnly(context) => {
                serde_json::to_string(context).expect("request-only context serializes")
            }
        })
        .collect()
}

/// The corrective-context marker of the malformed-proposal regeneration, as
/// it appears in a request's exact Effective System Prompt.
const CORRECTIVE_MARKER: &str = "[runtime generation feedback]";

/// The exact Effective System Prompt of every actual request, in order.
fn request_prompts(model: &Arc<FakeModel>) -> Vec<String> {
    model
        .requests()
        .iter()
        .map(|request| request.effective_system_prompt.clone())
        .collect()
}

/// Which actual requests carried the corrective hint, by ordinal.
fn corrective_requests(model: &Arc<FakeModel>) -> Vec<usize> {
    request_prompts(model)
        .iter()
        .enumerate()
        .filter(|(_, prompt)| prompt.contains(CORRECTIVE_MARKER))
        .map(|(ordinal, _)| ordinal)
        .collect()
}

fn assert_nothing_executed(audit: &common::DurableExecutionAudit) {
    assert!(
        tool_messages(audit).is_empty(),
        "a refused proposal never settles a ToolResult"
    );
    assert!(
        audit.event_history.iter().all(|event| !matches!(
            event,
            RuntimeEvent::ToolExecutionStarted { .. } | RuntimeEvent::ToolExecutionCompleted { .. }
        )),
        "a refused proposal never reaches the Tool lifecycle"
    );
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// The observed vLLM/Qwen class, seen from the Agent Loop: a generation that
/// leaked reserved tool markup cannot settle as a successful assistant turn.
/// It enters the bounded recovery, and a second malformed generation
/// terminates the attempt explicitly — with exactly two physical
/// generations, never a third.
#[tokio::test]
async fn two_malformed_generations_terminate_without_a_third() {
    let model = fake_model(vec![
        leaking_generation(),
        refused_generation(MalformedToolProposalSource::AdapterStructural),
        // A third script exists deliberately: if the loop ever regenerated
        // again, the request count below would catch it instead of the model
        // failing on an exhausted script.
        stop_generation("this generation must never be requested"),
    ]);
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let audit = run(&model, ToolRegistry::new(), &cancellation).await;

    assert_eq!(
        model.requests().len(),
        2,
        "one malformed generation authorizes exactly one regeneration"
    );
    assert_eq!(retry_schedules(&audit.event_history), 1);

    let terminal = assert_single_terminal(&audit.event_history);
    let RuntimeEvent::AttemptFailed {
        error: AttemptFailure::Model { error },
        ..
    } = terminal
    else {
        panic!("expected an explicit model-generation failure, got {terminal:?}");
    };
    assert_eq!(error.kind, ModelErrorKind::MalformedToolProposal);
    assert_eq!(
        error.malformed_tool_proposal,
        Some(MalformedToolProposalSource::AdapterStructural)
    );
    assert_outcome(
        &audit,
        &AttemptOutcome::Failed {
            error: AttemptFailure::Model {
                error: error.clone(),
            },
        },
    );

    assert_nothing_executed(&audit);
    assert!(
        assistant_texts(&audit).is_empty(),
        "no malformed generation may commit an Assistant message"
    );
    assert!(
        !canonical_text(&audit).contains("<parameter="),
        "malformed provider fragments must not enter canonical history"
    );
}

/// A provider that *declares* the call malformed enters exactly the same
/// generic recovery as an adapter-detected structural refusal: the runtime
/// path is keyed on the provider-independent class, not on the provenance.
#[tokio::test]
async fn a_provider_declared_malformed_call_enters_the_same_recovery() {
    let call = write_call(serde_json::json!({"path": "notes.txt"}));
    let model = fake_model(vec![
        refused_generation(MalformedToolProposalSource::ProviderDeclared),
        tool_generation(&call),
        stop_generation("written"),
    ]);
    let tool = FakeTool::new(write_tool_definition(), success_result("ok"));
    let mut tools = ToolRegistry::new();
    tool.register(&mut tools);
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let audit = run(&model, tools, &cancellation).await;

    assert!(matches!(
        audit.outcome,
        AttemptOutcome::Completed {
            finish_reason: ModelFinishReason::Stop
        }
    ));
    assert_eq!(retry_schedules(&audit.event_history), 1);
    assert_eq!(tool_messages(&audit).len(), 1);
}

/// A proposal refused before acceptance produces no canonical `ToolCall`, so
/// there is nothing to preflight, nothing to execute, and nothing to settle.
/// The regeneration's valid call then crosses the ordinary Tool path exactly
/// once, and the discarded generation is absent from canonical history.
#[tokio::test]
async fn malformed_then_valid_executes_the_tool_exactly_once() {
    let call = write_call(serde_json::json!({"path": "notes.txt"}));
    let model = fake_model(vec![
        leaking_generation(),
        tool_generation(&call),
        stop_generation("written"),
    ]);
    let tool = FakeTool::new(write_tool_definition(), success_result("ok"));
    let mut calls_seen = tool.calls();
    let mut tools = ToolRegistry::new();
    tool.register(&mut tools);
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let audit = run(&model, tools, &cancellation).await;

    assert!(matches!(
        audit.outcome,
        AttemptOutcome::Completed {
            finish_reason: ModelFinishReason::Stop
        }
    ));
    // Two physical generations for the first logical step, then the ordinary
    // tool→model continuation.
    assert_eq!(model.requests().len(), 3);
    assert_eq!(retry_schedules(&audit.event_history), 1);

    // The tool executed exactly once, through the ordinary path.
    assert_eq!(
        calls_seen.borrow_and_update().len(),
        1,
        "the accepted call executes exactly once"
    );
    let settled = tool_messages(&audit);
    assert_eq!(settled.len(), 1, "exactly one ToolResult settles");
    assert!(matches!(
        settled[0].result.status,
        ToolExecutionStatus::Success
    ));
    assert_eq!(
        audit
            .event_history
            .iter()
            .filter(|event| matches!(event, RuntimeEvent::ToolExecutionStarted { .. }))
            .count(),
        1
    );

    // The discarded generation left nothing canonical behind.
    assert_eq!(
        assistant_texts(&audit),
        vec![String::new(), "written".to_owned()],
        "only the accepted tool turn and the final answer are canonical"
    );
    assert!(!canonical_text(&audit).contains("<parameter="));
}

/// The discarded physical generation must not reappear in the *reconstructed*
/// conversation any later request receives. Both the malformed assistant
/// output and the corrective hint are checked: the first must never exist at
/// all, and the second must be carried by exactly the corrective generation
/// and by no request after it resolves.
///
/// The hint is noncanonical request-only context, which is not the same as
/// unpersisted context: the `RequestSnapshot` of the corrective request does
/// freeze the exact prompt that was sent, hint included, because audit
/// evidence must reproduce the real request. What must never happen is the
/// hint becoming *conversation* — a canonical message, or context for a
/// later step.
#[tokio::test]
async fn the_discarded_generation_never_reaches_a_later_request() {
    let call = write_call(serde_json::json!({"path": "notes.txt"}));
    let model = fake_model(vec![
        leaking_generation(),
        tool_generation(&call),
        stop_generation("written"),
    ]);
    let tool = FakeTool::new(write_tool_definition(), success_result("ok"));
    let mut tools = ToolRegistry::new();
    tool.register(&mut tools);
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let audit = run(&model, tools, &cancellation).await;

    assert!(
        !reconstructed_request_text(&model).contains("<parameter="),
        "the malformed generation must not be replayed to the model"
    );
    assert!(!canonical_text(&audit).contains("<parameter="));

    // The corrective context is owned by the corrective generation: the one
    // request realizing it carries the hint, and the tool→model continuation
    // that follows, being a new semantic generation, does not.
    let requests = model.requests();
    let prompts: Vec<&str> = requests
        .iter()
        .map(|request| request.effective_system_prompt.as_str())
        .collect();
    assert_eq!(prompts.len(), 3);
    assert!(
        !prompts[0].contains("[runtime generation feedback]"),
        "the first generation has no corrective context"
    );
    assert!(
        prompts[1].contains("[runtime generation feedback]"),
        "the regeneration carries exactly one corrective hint"
    );
    assert!(
        !prompts[2].contains("[runtime generation feedback]"),
        "the corrective hint does not survive its generation into later requests"
    );
    // The corrective hint is provider-independent guidance plus a bounded
    // reason; it never carries the malformed output itself as an example.
    assert!(!prompts[1].contains("notes.txt\n</parameter>"));
}

/// A structurally valid canonical `ToolCall` whose JSON violates the declared
/// Tool schema is Tool business, not model-generation business: it crosses
/// acceptance, is rejected by preflight, and settles as a failed
/// `ToolResult`. It must not consume the malformed-generation budget or
/// cause any regeneration.
#[tokio::test]
async fn schema_rejection_stays_on_the_tool_path() {
    let call = write_call(serde_json::json!({"unexpected": true}));
    let model = fake_model(vec![tool_generation(&call), stop_generation("recovered")]);
    let tool = FakeTool::new(write_tool_definition(), success_result("unexpected"));
    let mut calls_seen = tool.calls();
    let mut tools = ToolRegistry::new();
    let mut definition = write_tool_definition();
    definition.input_schema = serde_json::json!({
        "type": "object",
        "properties": {"path": {"type": "string"}},
        "required": ["path"],
        "additionalProperties": false
    });
    tools
        .register(definition, Arc::new(tool))
        .expect("register the strict tool");
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let audit = run(&model, tools, &cancellation).await;

    assert!(matches!(
        audit.outcome,
        AttemptOutcome::Completed {
            finish_reason: ModelFinishReason::Stop
        }
    ));
    assert_eq!(
        model.requests().len(),
        2,
        "a schema rejection causes no regeneration"
    );
    assert_eq!(
        retry_schedules(&audit.event_history),
        0,
        "a schema rejection never enters model-generation recovery"
    );
    assert!(
        audit
            .event_history
            .iter()
            .all(|event| !matches!(event, RuntimeEvent::ModelRequestFailed { .. })),
        "the generation itself succeeded"
    );

    // The call crossed acceptance and settled through the ordinary contract.
    let settled = tool_messages(&audit);
    assert_eq!(settled.len(), 1);
    assert!(matches!(
        settled[0].result.status,
        ToolExecutionStatus::Failed { .. }
    ));
    assert!(
        calls_seen.borrow_and_update().is_empty(),
        "preflight rejected the call before the executor"
    );
    assert!(
        !model.requests()[0]
            .effective_system_prompt
            .contains("[runtime generation feedback]")
    );
    assert!(
        !model.requests()[1]
            .effective_system_prompt
            .contains("[runtime generation feedback]")
    );
}

/// The malformed-proposal budget composes additively with the transient
/// budget and is never reset by it. A transient retry between two malformed
/// generations does not restore the spent regeneration, so the second
/// malformed generation terminates the attempt at three physical
/// generations rather than starting a fourth.
#[tokio::test]
async fn a_transient_retry_does_not_restore_the_malformed_budget() {
    let model = fake_model(vec![
        refused_generation(MalformedToolProposalSource::StreamAssembly),
        vec![
            FakeStep::Emit(ModelEvent::Started),
            FakeStep::Emit(transient("temporary throttle")),
        ],
        refused_generation(MalformedToolProposalSource::StreamAssembly),
        stop_generation("this generation must never be requested"),
    ]);
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let audit = run(&model, ToolRegistry::new(), &cancellation).await;

    assert_eq!(
        model.requests().len(),
        3,
        "one malformed regeneration plus one transient retry, and no more"
    );
    let terminal = assert_single_terminal(&audit.event_history);
    let RuntimeEvent::AttemptFailed {
        error: AttemptFailure::Model { error },
        ..
    } = terminal
    else {
        panic!("expected an explicit model-generation failure, got {terminal:?}");
    };
    assert_eq!(error.kind, ModelErrorKind::MalformedToolProposal);
    assert_nothing_executed(&audit);
}

/// The inverse composition: a malformed generation does not consume, extend,
/// or multiply the transient budget. The transient budget still allows its
/// full three retries afterwards, and the attempt then fails on the
/// transient error rather than looping.
#[tokio::test]
async fn a_malformed_regeneration_does_not_multiply_the_transient_budget() {
    let mut scripts = vec![refused_generation(
        MalformedToolProposalSource::AdapterStructural,
    )];
    for ordinal in 0..4 {
        scripts.push(vec![
            FakeStep::Emit(ModelEvent::Started),
            FakeStep::Emit(transient(&format!("temporary throttle {ordinal}"))),
        ]);
    }
    scripts.push(stop_generation("this generation must never be requested"));
    let model = fake_model(scripts);
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let audit = run(&model, ToolRegistry::new(), &cancellation).await;

    assert_eq!(
        model.requests().len(),
        5,
        "one malformed generation, its single regeneration, and the three \
         transient retries the generic budget already allowed"
    );
    let terminal = assert_single_terminal(&audit.event_history);
    let RuntimeEvent::AttemptFailed {
        error: AttemptFailure::Model { error },
        ..
    } = terminal
    else {
        panic!("expected a model failure, got {terminal:?}");
    };
    assert_eq!(
        error.kind,
        ModelErrorKind::RateLimit,
        "the terminal is the exhausted transient budget, not a malformed loop"
    );
    assert_nothing_executed(&audit);
}

/// Cancellation observed before the corrective generation begins wins: a
/// remaining regeneration budget never authorizes another provider request
/// after cancellation is observable.
///
/// The cut is exact. The first stream parks *after* yielding its malformed
/// terminal, so the loop has already captured that terminal when the
/// controller cancels; the loop therefore reaches the malformed-recovery
/// branch with a full budget and with cancellation already observable.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancellation_before_the_regeneration_wins_over_the_budget() {
    let (release, release_rx) = support::fake::model_release();
    let mut first = leaking_generation();
    first.push(FakeStep::ParkUntilReleased(release_rx));
    let model = fake_model(vec![
        first,
        stop_generation("this generation must never be requested"),
    ]);
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);

    let controller_cancellation = cancellation.clone();
    let mut parked = model.parked();
    let controller = tokio::spawn(async move {
        parked
            .wait_for(|is_parked| *is_parked)
            .await
            .expect("the model park watch stays open");
        controller_cancellation.cancel();
        release.send_replace(true);
    });

    let audit = run(&model, ToolRegistry::new(), &cancellation).await;
    controller.await.expect("controller task");

    assert_eq!(
        model.requests().len(),
        1,
        "no model request may start after cancellation is observable"
    );
    assert!(
        matches!(
            audit.outcome,
            AttemptOutcome::Cancelled {
                reason: CancellationReason::UserRequested
            }
        ),
        "unexpected outcome: {:?}",
        audit.outcome
    );
    assert_single_terminal(&audit.event_history);
    assert_nothing_executed(&audit);
    assert!(assistant_texts(&audit).is_empty());
}

/// The corrective hint belongs to the corrective **generation**, not to one
/// actual request.
///
/// A malformed generation authorizes exactly one corrective generation. The
/// first actual request that tries to realize it fails in transport before
/// producing any generation at all, so the transient retry that follows is
/// still the *same* semantic generation and must carry the same corrective
/// hint. A hint consumed at request-construction time would silently drop
/// here, and the model would be asked to regenerate the step with no idea
/// why its previous answer was discarded.
///
/// ```text
/// #0  malformed generation
/// #1  corrective generation, attempt 1 -> transport/rate-limit failure
/// #2  corrective generation, attempt 2 -> valid ToolCall
/// #3  ordinary tool-result continuation
/// ```
#[tokio::test]
async fn the_corrective_hint_survives_a_transient_actual_request_failure() {
    let call = write_call(serde_json::json!({"path": "notes.txt"}));
    let model = fake_model(vec![
        leaking_generation(),
        vec![
            FakeStep::Emit(ModelEvent::Started),
            FakeStep::Emit(transient("temporary throttle")),
        ],
        tool_generation(&call),
        stop_generation("written"),
    ]);
    let tool = FakeTool::new(write_tool_definition(), success_result("ok"));
    let mut calls_seen = tool.calls();
    let mut tools = ToolRegistry::new();
    tool.register(&mut tools);
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let audit = run(&model, tools, &cancellation).await;

    // Both actual requests of the corrective generation carry the hint; the
    // malformed generation before it and the continuation after it do not.
    assert_eq!(request_prompts(&model).len(), 4);
    assert_eq!(
        corrective_requests(&model),
        vec![1, 2],
        "every actual request realizing the corrective generation carries the hint"
    );

    // Exactly one semantic regeneration was authorized: the transient retry
    // is one more attempt at it, never a second corrective generation, so
    // the step settled in four actual requests rather than looping.
    assert_eq!(model.requests().len(), 4);

    // The discarded malformed output never became context, in either
    // direction.
    assert!(!reconstructed_request_text(&model).contains("<parameter="));
    assert!(!canonical_text(&audit).contains("<parameter="));
    let hint_request = &request_prompts(&model)[2];
    assert!(!hint_request.contains("notes.txt\n</parameter>"));

    // The corrective generation's valid call settles exactly once through
    // the ordinary Tool path.
    assert_eq!(
        calls_seen.borrow_and_update().len(),
        1,
        "the accepted call executes exactly once"
    );
    let settled = tool_messages(&audit);
    assert_eq!(settled.len(), 1, "exactly one ToolResult settles");
    assert!(matches!(
        settled[0].result.status,
        ToolExecutionStatus::Success
    ));
    assert_outcome(
        &audit,
        &AttemptOutcome::Completed {
            finish_reason: ModelFinishReason::Stop,
        },
    );
}

/// The same invariant across the other recovery that produces further actual
/// requests of one semantic generation: a context-overflow rejection.
///
/// The overflow interrupts the corrective generation before it produced any
/// model outcome. Compaction rebuilds a smaller frozen state and issues
/// another actual request — still the same corrective generation, so it must
/// still carry the corrective hint.
///
/// ```text
/// #0  malformed generation
/// #1  corrective generation, attempt 1 -> context window exceeded
/// #2  corrective generation, attempt 2 (post-compaction) -> ordinary answer
/// ```
#[tokio::test]
async fn the_corrective_hint_survives_context_overflow_recovery() {
    let model = fake_model(vec![
        leaking_generation(),
        vec![
            FakeStep::Emit(ModelEvent::Started),
            FakeStep::Emit(overflow("provider context window exceeded")),
        ],
        stop_generation("answered without a tool"),
    ]);
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let audit = run_with_runtime(
        &model,
        ToolRegistry::new(),
        &cancellation,
        compacting_runtime(),
    )
    .await;

    assert_eq!(request_prompts(&model).len(), 3);
    assert_eq!(
        corrective_requests(&model),
        vec![1, 2],
        "compaction recovers an actual request, not the corrective generation"
    );
    assert!(!canonical_text(&audit).contains("<parameter="));
    assert_outcome(
        &audit,
        &AttemptOutcome::Completed {
            finish_reason: ModelFinishReason::Stop,
        },
    );
}

/// The other end of the lifetime: the hint is cleared the moment the
/// corrective generation reaches a canonical model outcome.
///
/// The corrective generation here answers with a valid tool call, so it
/// resolves. Everything after that — the tool-result continuation, and the
/// next logical model step it starts — is ordinary work that must not be
/// told to regenerate anything. A hint that is merely "one request old"
/// would survive into the continuation; one owned by the resolved generation
/// cannot.
#[tokio::test]
async fn the_corrective_hint_is_cleared_when_its_generation_resolves() {
    let call = write_call(serde_json::json!({"path": "notes.txt"}));
    let model = fake_model(vec![
        refused_generation(MalformedToolProposalSource::AdapterStructural),
        tool_generation(&call),
        stop_generation("written"),
    ]);
    let tool = FakeTool::new(write_tool_definition(), success_result("ok"));
    let mut calls_seen = tool.calls();
    let mut tools = ToolRegistry::new();
    tool.register(&mut tools);
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let audit = run(&model, tools, &cancellation).await;

    assert_eq!(
        corrective_requests(&model),
        vec![1],
        "only the corrective generation carries the hint"
    );
    assert!(
        !request_prompts(&model)[2].contains(MALFORMED_MARKER),
        "no fragment of the corrective reason survives into the continuation"
    );
    assert_eq!(
        calls_seen.borrow_and_update().len(),
        1,
        "the accepted call executes exactly once"
    );
    assert_outcome(
        &audit,
        &AttemptOutcome::Completed {
            finish_reason: ModelFinishReason::Stop,
        },
    );
}
