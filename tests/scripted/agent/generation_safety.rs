//! Issue #203: single-generation liveness, budget, and integrity in the
//! Agent Loop.
//!
//! Three contracts meet at one boundary and this suite owns the
//! provider-independent half of the last two:
//!
//! ```text
//! transport liveness   !=   generation budget   !=   generation integrity
//! (agent::deadlines)        (here)                   (here)
//! ```
//!
//! What is proven here is the *logical model step*: a physical generation
//! that degenerated, exhausted a budget, or was truncated at the provider's
//! token limit is discarded before the canonical commit boundary; the step
//! gets exactly **one** semantic corrective generation, shared with the
//! malformed-tool-proposal recovery of Issue #201; a second semantic anomaly
//! of any class terminates the attempt; and nothing from a discarded
//! generation reaches canonical history or a later request reconstruction.
//!
//! The detector's own algorithm — its evidence threshold, chunk-boundary
//! invariance, channel attribution, and false-positive controls — is owned by
//! the unit tests in `src/model/generation.rs`. Nothing here knows a provider
//! protocol, and nothing here depends on elapsed time: every cut is a
//! scripted stream or an explicit synchronization channel.

use super::super::{common, support};

use std::sync::Arc;
use std::time::Duration;

use rustx::agent::{AgentCancellation, AgentExecution, AgentExecutionRequest};
use rustx::events::types::{AttemptFailure, AttemptOutcome, RuntimeEvent};
use rustx::message::content::TextBlock;
use rustx::message::types::{
    AssistantContentBlock, ContentBlockIndex, InboundKind, MessageBlock, ToolMessageBlock,
    UserContentBlock, UserMessageBlock, UserSource,
};
use rustx::model::event::ModelEvent;
use rustx::model::finish::ModelFinishReason;
use rustx::model::{
    GenerationBudgetKind, GenerationChannel, GenerationFailure, GenerationSafetyPolicy,
    MalformedToolProposalSource, ModelError, ModelErrorKind, ModelInputMessage,
    ModelRetryDisposition,
};
use rustx::runtime::identity::{AgentId, AttemptId, ConversationId, MessageId};
use rustx::runtime::types::CancellationReason;
use rustx::tools::executor::ToolRegistry;
use rustx::tools::types::{ToolConcurrencyPolicy, ToolExecutionPolicy, ToolExecutionStatus};
use support::audit::{assert_outcome, assert_single_terminal};
use support::fake::{
    FakeModel, FakeStep, FakeTool, ScriptedCall, fake_model, success_result, tool_call_events,
};

const CONVERSATION: &str = "conv-203";

/// The repeating unit of every degenerate fixture. It is a marker so the
/// discarded output can be searched for in canonical history and in every
/// later request reconstruction.
const LOOP_UNIT: &str = "LOOPMARK ";

/// The corrective-context marker of a semantic corrective generation, as it
/// appears in a request's exact Effective System Prompt.
const CORRECTIVE_MARKER: &str = "[runtime generation feedback]";

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

fn request(model: &Arc<FakeModel>) -> AgentExecutionRequest {
    AgentExecutionRequest {
        agent_id: AgentId::new("agent-203"),
        conversation_id: ConversationId::new(CONVERSATION),
        attempt_id: AttemptId::new("attempt-203"),
        conversation: rustx::conversation::ConversationState::from_messages(vec![
            MessageBlock::User(UserMessageBlock {
                id: MessageId::new("msg-user-1"),
                content: vec![UserContentBlock::Text(TextBlock {
                    text: "explain the plan".to_owned(),
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

/// The generation-safety policy installed by the budget regressions.
///
/// Production resolves the policy at the model-invocation layer, where it is
/// a runaway backstop measured in hundreds of kilobytes. A regression that
/// proves the *enforcement* does not need to stream that much through the
/// publication and durability planes, so it installs an explicit policy
/// exactly as the deadline suites install an explicit timeout policy.
///
/// The two bounds here are deliberately unrelated to each other — 4096 and
/// 512 stand in no ratio the runtime knows about — which is itself the point:
/// total output and reasoning are independent inputs of the policy, not one
/// derived from the other. The reasoning bound also sits below the repetition
/// detector's evidence threshold, so a budget regression cannot pass by
/// accidentally tripping the detector instead.
fn small_policy() -> GenerationSafetyPolicy {
    GenerationSafetyPolicy {
        max_generated_bytes: 4_096,
        max_reasoning_bytes: Some(512),
    }
}

struct Setup {
    tools: ToolRegistry,
    policy: Option<GenerationSafetyPolicy>,
}

impl Setup {
    fn new() -> Self {
        Self {
            tools: ToolRegistry::new(),
            policy: None,
        }
    }

    fn with_tools(mut self, tools: ToolRegistry) -> Self {
        self.tools = tools;
        self
    }

    const fn with_policy(mut self, policy: GenerationSafetyPolicy) -> Self {
        self.policy = Some(policy);
        self
    }
}

async fn run_with(
    model: &Arc<FakeModel>,
    cancellation: &AgentCancellation,
    setup: Setup,
) -> common::DurableExecutionAudit {
    let tool_runtime = common::tool_runtime(CONVERSATION);
    let store = tool_runtime.durable_store();
    let capability = common::capability_lease(setup.tools, &tool_runtime).await;
    let mut execution = AgentExecution::new(
        request(model),
        capability.into_lease(),
        cancellation,
        support::default_execution_policy(),
        runtime(model),
        &tool_runtime,
        rustx::agent::AttemptLifecycle::inert(),
    )
    .expect("conversation identity matches the tool runtime");
    if let Some(policy) = setup.policy {
        execution.install_generation_policy(policy);
    }
    let result = execution.run().await;
    common::durable_agent_result(result, store.as_ref())
}

async fn run(
    model: &Arc<FakeModel>,
    cancellation: &AgentCancellation,
) -> common::DurableExecutionAudit {
    run_with(model, cancellation, Setup::new()).await
}

// ---------------------------------------------------------------------------
// Scripted generations
// ---------------------------------------------------------------------------

fn text_delta(text: &str) -> ModelEvent {
    text_delta_at(0, text)
}

fn text_delta_at(block: u32, text: &str) -> ModelEvent {
    ModelEvent::TextDelta {
        block_index: ContentBlockIndex::new(block),
        text: text.to_owned(),
    }
}

fn reasoning_delta(text: &str) -> ModelEvent {
    ModelEvent::ReasoningDelta {
        block_index: ContentBlockIndex::new(0),
        text: text.to_owned(),
    }
}

fn completed(finish_reason: ModelFinishReason) -> ModelEvent {
    ModelEvent::Completed {
        finish_reason,
        usage: None,
    }
}

/// A generation whose visible content collapses into a short-period loop.
///
/// The units arrive as separate provider deltas so the fixture also exercises
/// the loop's per-delta path rather than one convenient blob.
fn degenerate_content_generation() -> Vec<FakeStep> {
    let mut steps = vec![FakeStep::Emit(ModelEvent::Started)];
    steps.push(FakeStep::Emit(text_delta("Here is the plan. ")));
    for _ in 0..200 {
        steps.push(FakeStep::Emit(text_delta(LOOP_UNIT)));
    }
    // Never reached: the loop drops the stream at classification.
    steps.push(FakeStep::Emit(completed(ModelFinishReason::Stop)));
    steps
}

/// A generation whose reasoning collapses into a short-period loop while its
/// visible content stays perfectly ordinary.
fn degenerate_reasoning_generation() -> Vec<FakeStep> {
    // Reasoning is block 0 and visible content is block 1, which is what the
    // canonical assembler requires and what makes the attribution claim
    // meaningful: both channels stream, only one of them repeats.
    let mut steps = vec![FakeStep::Emit(ModelEvent::Started)];
    for index in 0..200 {
        steps.push(FakeStep::Emit(reasoning_delta(LOOP_UNIT)));
        steps.push(FakeStep::Emit(text_delta_at(1, &format!("step {index}. "))));
    }
    steps.push(FakeStep::Emit(completed(ModelFinishReason::Stop)));
    steps
}

/// A generation that thinks continuously without ever repeating itself.
fn endless_reasoning_generation() -> Vec<FakeStep> {
    let mut steps = vec![FakeStep::Emit(ModelEvent::Started)];
    for index in 0..400 {
        steps.push(FakeStep::Emit(reasoning_delta(&format!(
            "considering option {index} of the plan; "
        ))));
    }
    steps.push(FakeStep::Emit(completed(ModelFinishReason::Stop)));
    steps
}

/// A generation the provider itself truncated at its token limit.
fn truncated_generation() -> Vec<FakeStep> {
    vec![
        FakeStep::Emit(ModelEvent::Started),
        FakeStep::Emit(text_delta("TRUNCATED-HALF-SENTENCE that stops mid-")),
        FakeStep::Emit(completed(ModelFinishReason::Length)),
    ]
}

fn answer_generation(text: &str) -> Vec<FakeStep> {
    vec![
        FakeStep::Emit(ModelEvent::Started),
        FakeStep::Emit(text_delta(text)),
        FakeStep::Emit(completed(ModelFinishReason::Stop)),
    ]
}

fn malformed_generation() -> Vec<FakeStep> {
    vec![
        FakeStep::Emit(ModelEvent::Started),
        FakeStep::Emit(ModelEvent::Failed {
            error: ModelError::malformed_tool_proposal(
                MalformedToolProposalSource::StreamAssembly,
                "the model tool proposal carries no usable invocation id",
            ),
        }),
    ]
}

fn transient_generation() -> Vec<FakeStep> {
    vec![
        FakeStep::Emit(ModelEvent::Started),
        FakeStep::Emit(ModelEvent::Failed {
            error: ModelError {
                kind: ModelErrorKind::RateLimit,
                message: "provider rate limit".to_owned(),
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
        }),
    ]
}

/// A provider-reported request timeout.
///
/// It is a *transport* failure: the request did not produce a usable
/// generation because the provider stopped responding, which says nothing
/// about the integrity of anything it did produce. A provider-reported
/// timeout carries no `timeout_phase`, because this runtime observed no
/// deadline of its own; the runtime-owned deadline path, which does carry
/// one, is proven in `scripted_suites::agent::deadlines`.
fn provider_timeout_generation() -> Vec<FakeStep> {
    vec![
        FakeStep::Emit(ModelEvent::Started),
        FakeStep::Emit(ModelEvent::Failed {
            error: ModelError {
                kind: ModelErrorKind::Timeout,
                message: "the provider did not respond in time".to_owned(),
                retry_disposition: ModelRetryDisposition::Transient,
                retry_after_ms: Some(0),
                provider_code: Some("timeout".to_owned()),
                context_overflow: None,
                malformed_tool_proposal: None,
                timeout_phase: None,
                generation: None,
            },
        }),
    ]
}

fn tool_generation(call: &ScriptedCall) -> Vec<FakeStep> {
    let mut steps = vec![FakeStep::Emit(ModelEvent::Started)];
    for event in tool_call_events(0, call) {
        steps.push(FakeStep::Emit(event));
    }
    steps.push(FakeStep::Emit(completed(ModelFinishReason::ToolCalls)));
    steps
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

/// Every canonical history fact of the attempt, as one haystack.
fn canonical_text(audit: &common::DurableExecutionAudit) -> String {
    audit
        .messages()
        .iter()
        .map(|message| serde_json::to_string(message).expect("canonical message serializes"))
        .collect()
}

/// Every message the model was ever asked to reason over, across all physical
/// requests: the reconstruction projection a later turn actually receives.
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

fn request_prompts(model: &Arc<FakeModel>) -> Vec<String> {
    model
        .requests()
        .iter()
        .map(|request| request.effective_system_prompt.clone())
        .collect()
}

/// Which actual requests carried a corrective hint, by ordinal.
fn corrective_requests(model: &Arc<FakeModel>) -> Vec<usize> {
    request_prompts(model)
        .iter()
        .enumerate()
        .filter(|(_, prompt)| prompt.contains(CORRECTIVE_MARKER))
        .map(|(ordinal, _)| ordinal)
        .collect()
}

/// The typed generation facts of every durable `ModelRequestFailed`, in order.
fn generation_failures(audit: &common::DurableExecutionAudit) -> Vec<(ModelErrorKind, ModelError)> {
    audit
        .event_history
        .iter()
        .filter_map(|event| match event {
            RuntimeEvent::ModelRequestFailed { error, .. } => {
                Some((error.kind.clone(), error.clone()))
            }
            _ => None,
        })
        .collect()
}

fn completed_requests(audit: &common::DurableExecutionAudit) -> usize {
    audit
        .event_history
        .iter()
        .filter(|event| matches!(event, RuntimeEvent::ModelRequestCompleted { .. }))
        .count()
}

fn assert_nothing_executed(audit: &common::DurableExecutionAudit) {
    assert!(
        tool_messages(audit).is_empty(),
        "a discarded generation never settles a ToolResult"
    );
    assert!(
        audit.event_history.iter().all(|event| !matches!(
            event,
            RuntimeEvent::ToolExecutionStarted { .. } | RuntimeEvent::ToolExecutionCompleted { .. }
        )),
        "a discarded generation never reaches the Tool lifecycle"
    );
}

/// The bounded diagnostic contract: the typed fact carries only integers and
/// enumerations, and the rendered message is runtime-authored, short, and
/// free of the discarded output.
fn assert_bounded_diagnostic(error: &ModelError) {
    assert!(
        error.message.len() < 256,
        "a generation diagnostic is bounded at construction: {}",
        error.message
    );
    assert!(
        !error.message.contains(LOOP_UNIT),
        "a generation diagnostic never echoes the discarded output"
    );
    let payload = serde_json::to_value(error.generation.expect("typed generation fact"))
        .expect("the typed fact serializes");
    let object = payload.as_object().expect("the typed fact is an object");
    assert!(
        object
            .values()
            .all(|value| value.is_number() || value.is_string()),
        "the typed fact carries only numbers and stable discriminators: {payload}"
    );
}

// ---------------------------------------------------------------------------
// Generation integrity
// ---------------------------------------------------------------------------

/// A degenerated first generation is discarded whole, the step is regenerated
/// once, and only the corrective generation commits.
///
/// This is the Issue #203 core: the failed generation streamed visible deltas
/// that observers already saw, and none of it is canonical.
#[tokio::test]
async fn a_degenerate_generation_is_discarded_and_regenerated_once() {
    let model = fake_model(vec![
        degenerate_content_generation(),
        answer_generation("the plan is to ship it"),
        // A third script exists deliberately: an extra semantic generation
        // would show up as a request count of three rather than as a model
        // failing on an exhausted script.
        answer_generation("this generation must never be requested"),
    ]);
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let audit = run(&model, &cancellation).await;

    assert_eq!(
        model.requests().len(),
        2,
        "one degenerate generation authorizes exactly one corrective generation"
    );
    assert_outcome(
        &audit,
        &AttemptOutcome::Completed {
            finish_reason: ModelFinishReason::Stop,
        },
    );

    // Exactly one canonical Assistant message, and it is the corrective one.
    assert_eq!(assistant_texts(&audit), vec!["the plan is to ship it"]);
    assert!(
        !canonical_text(&audit).contains(LOOP_UNIT),
        "the discarded generation must not enter canonical history"
    );
    assert!(
        !reconstructed_request_text(&model).contains(LOOP_UNIT),
        "the discarded generation must not be replayed to the model"
    );
    assert_eq!(
        completed_requests(&audit),
        1,
        "one accepted provider outcome"
    );

    // The typed provider-independent fact, with channel attribution.
    let failures = generation_failures(&audit);
    assert_eq!(failures.len(), 1);
    assert_eq!(failures[0].0, ModelErrorKind::GenerationDegenerated);
    let GenerationFailure::Degenerated {
        channel,
        period_bytes,
        repetitions,
        span_bytes,
    } = failures[0].1.generation.expect("typed generation fact")
    else {
        panic!("expected a degeneration fact");
    };
    assert_eq!(channel, GenerationChannel::Content);
    assert_eq!(
        usize::try_from(period_bytes).expect("period fits"),
        LOOP_UNIT.len()
    );
    assert!(repetitions >= 4);
    assert!(span_bytes >= 1_024);
    assert_bounded_diagnostic(&failures[0].1);

    // The corrective hint belongs to exactly the corrective generation.
    assert_eq!(corrective_requests(&model), vec![1]);
    assert!(
        !request_prompts(&model)[1].contains(LOOP_UNIT),
        "the correction states the failure class, never the discarded output"
    );
}

/// Degeneration in reasoning is attributed to the reasoning channel even
/// while the visible content of the same generation is perfectly ordinary.
#[tokio::test]
async fn reasoning_degeneration_is_attributed_to_the_reasoning_channel() {
    let model = fake_model(vec![
        degenerate_reasoning_generation(),
        answer_generation("answered without looping"),
    ]);
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let audit = run(&model, &cancellation).await;

    let failures = generation_failures(&audit);
    assert_eq!(failures.len(), 1);
    let GenerationFailure::Degenerated { channel, .. } =
        failures[0].1.generation.expect("typed generation fact")
    else {
        panic!("expected a degeneration fact");
    };
    assert_eq!(channel, GenerationChannel::Reasoning);
    assert_eq!(assistant_texts(&audit), vec!["answered without looping"]);
    assert!(!canonical_text(&audit).contains(LOOP_UNIT));
}

/// Degeneration is recognized from repetition evidence alone, long before any
/// output budget is relevant: the attempt runs under a budget far larger than
/// the bytes the degenerate generation ever produced.
#[tokio::test]
async fn degeneration_is_detected_before_the_generation_budget_is_consumed() {
    let model = fake_model(vec![
        degenerate_content_generation(),
        answer_generation("recovered"),
    ]);
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let audit = run_with(
        &model,
        &cancellation,
        // The whole degenerate fixture is 1_818 bytes; the budget is larger,
        // so a budget classification here would be a bug rather than a race.
        Setup::new().with_policy(GenerationSafetyPolicy {
            max_generated_bytes: 8_192,
            max_reasoning_bytes: Some(8_192),
        }),
    )
    .await;

    let failures = generation_failures(&audit);
    assert_eq!(failures.len(), 1);
    assert_eq!(failures[0].0, ModelErrorKind::GenerationDegenerated);
    assert_eq!(assistant_texts(&audit), vec!["recovered"]);
}

/// Structured tool-call arguments are wire data, not generated text: a
/// deliberately repetitive argument stream crosses `ToolCall` acceptance and
/// executes through the ordinary Tool path.
#[tokio::test]
async fn tool_argument_streams_are_not_inspected_as_degenerate_text() {
    let repetitive = serde_json::json!({
        "path": "table.json",
        "rows": vec![serde_json::json!({"a": [], "b": [], "c": []}); 200],
    });
    let call = ScriptedCall {
        id: "call-203",
        tool_id: "tool-write",
        name: "write_file",
        arguments: repetitive,
    };
    let model = fake_model(vec![
        tool_generation(&call),
        answer_generation("table written"),
    ]);
    let tool = FakeTool::new(write_tool_definition(), success_result("ok"));
    let mut calls_seen = tool.calls();
    let mut tools = ToolRegistry::new();
    tool.register(&mut tools);
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let audit = run_with(&model, &cancellation, Setup::new().with_tools(tools)).await;

    assert!(
        generation_failures(&audit).is_empty(),
        "repetitive tool arguments are not a degenerate generation"
    );
    assert_eq!(calls_seen.borrow_and_update().len(), 1);
    assert_eq!(tool_messages(&audit).len(), 1);
    assert!(matches!(
        tool_messages(&audit)[0].result.status,
        ToolExecutionStatus::Success
    ));
    assert_outcome(
        &audit,
        &AttemptOutcome::Completed {
            finish_reason: ModelFinishReason::Stop,
        },
    );
}

// ---------------------------------------------------------------------------
// Generation budget
// ---------------------------------------------------------------------------

/// Continuous, never-repeating reasoning is perfectly live and never trips
/// the repetition detector. It is bounded by the reasoning budget alone, and
/// the resulting fact names the reasoning budget rather than the total.
#[tokio::test]
async fn continuous_reasoning_cannot_evade_the_reasoning_budget() {
    let model = fake_model(vec![
        endless_reasoning_generation(),
        answer_generation("answered concisely"),
    ]);
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let audit = run_with(
        &model,
        &cancellation,
        Setup::new().with_policy(small_policy()),
    )
    .await;

    let failures = generation_failures(&audit);
    assert_eq!(failures.len(), 1);
    assert_eq!(failures[0].0, ModelErrorKind::GenerationBudgetExceeded);
    let GenerationFailure::RuntimeBudgetExceeded {
        budget,
        limit_bytes,
        observed_bytes,
    } = failures[0].1.generation.expect("typed generation fact")
    else {
        panic!("expected a runtime budget fact");
    };
    assert_eq!(budget, GenerationBudgetKind::Reasoning);
    assert_eq!(
        u64::from(limit_bytes),
        small_policy()
            .max_reasoning_bytes
            .expect("a reasoning bound"),
        "the fact names the reasoning bound, never the total"
    );
    assert!(
        u64::from(observed_bytes)
            > small_policy()
                .max_reasoning_bytes
                .expect("a reasoning bound")
    );
    assert_bounded_diagnostic(&failures[0].1);

    // The over-budget generation is discarded; only the corrective one
    // commits, and the reasoning it produced is nowhere canonical.
    assert_eq!(model.requests().len(), 2);
    assert_eq!(assistant_texts(&audit), vec!["answered concisely"]);
    assert!(!canonical_text(&audit).contains("considering option"));
    assert!(!reconstructed_request_text(&model).contains("considering option"));
}

/// A generation the provider itself truncated at its token limit is
/// incomplete, so it never becomes an ordinary successful assistant
/// completion: the half sentence is discarded and only the complete
/// regeneration commits.
#[tokio::test]
async fn provider_length_termination_never_commits_a_truncated_turn() {
    let model = fake_model(vec![
        truncated_generation(),
        answer_generation("a complete answer"),
    ]);
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let audit = run(&model, &cancellation).await;

    let failures = generation_failures(&audit);
    assert_eq!(failures.len(), 1);
    assert_eq!(failures[0].0, ModelErrorKind::GenerationBudgetExceeded);
    assert_eq!(
        failures[0].1.generation,
        Some(GenerationFailure::ProviderLengthLimit),
        "the provider knows the exact token count and this runtime does not, \
         so the fact carries no invented measurement"
    );
    assert_eq!(
        completed_requests(&audit),
        1,
        "a truncated generation is not a completed provider outcome"
    );
    assert_eq!(assistant_texts(&audit), vec!["a complete answer"]);
    assert!(!canonical_text(&audit).contains("TRUNCATED-HALF-SENTENCE"));
    assert!(!reconstructed_request_text(&model).contains("TRUNCATED-HALF-SENTENCE"));
    assert_outcome(
        &audit,
        &AttemptOutcome::Completed {
            finish_reason: ModelFinishReason::Stop,
        },
    );
}

/// Two provider truncations terminate the step explicitly rather than
/// committing the second half sentence.
#[tokio::test]
async fn two_truncated_generations_terminate_without_a_third() {
    let model = fake_model(vec![
        truncated_generation(),
        truncated_generation(),
        answer_generation("this generation must never be requested"),
    ]);
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let audit = run(&model, &cancellation).await;

    assert_eq!(model.requests().len(), 2);
    let terminal = assert_single_terminal(&audit.event_history);
    let RuntimeEvent::AttemptFailed {
        error: AttemptFailure::Model { error },
        ..
    } = terminal
    else {
        panic!("expected an explicit model-generation failure, got {terminal:?}");
    };
    assert_eq!(error.kind, ModelErrorKind::GenerationBudgetExceeded);
    assert!(assistant_texts(&audit).is_empty());
    assert_nothing_executed(&audit);
}

// ---------------------------------------------------------------------------
// One shared semantic corrective-generation budget
// ---------------------------------------------------------------------------

/// A second semantic anomaly of the same class terminates the step: there is
/// no third semantic generation.
#[tokio::test]
async fn two_degenerate_generations_terminate_without_a_third() {
    let model = fake_model(vec![
        degenerate_content_generation(),
        degenerate_content_generation(),
        answer_generation("this generation must never be requested"),
    ]);
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let audit = run(&model, &cancellation).await;

    assert_eq!(
        model.requests().len(),
        2,
        "the spent corrective budget cannot authorize a third generation"
    );
    let terminal = assert_single_terminal(&audit.event_history);
    let RuntimeEvent::AttemptFailed {
        error: AttemptFailure::Model { error },
        ..
    } = terminal
    else {
        panic!("expected an explicit model-generation failure, got {terminal:?}");
    };
    assert_eq!(error.kind, ModelErrorKind::GenerationDegenerated);
    assert_outcome(
        &audit,
        &AttemptOutcome::Failed {
            error: AttemptFailure::Model {
                error: error.clone(),
            },
        },
    );
    assert!(
        assistant_texts(&audit).is_empty(),
        "no discarded generation commits an Assistant message"
    );
    assert!(!canonical_text(&audit).contains(LOOP_UNIT));
    assert_nothing_executed(&audit);
}

/// The Issue #201 malformed-proposal recovery and the Issue #203 degeneration
/// recovery are the same budget, in either order.
///
/// ```text
/// malformed     -> corrective generation -> degeneration -> terminal
/// degeneration  -> corrective generation -> malformed    -> terminal
/// ```
///
/// Neither ordering can reach a third semantic generation, which is exactly
/// what a per-class budget would have allowed.
#[tokio::test]
async fn malformed_and_degeneration_share_one_semantic_budget() {
    let orderings = [
        (
            "malformed then degeneration",
            vec![
                malformed_generation(),
                degenerate_content_generation(),
                answer_generation("this generation must never be requested"),
            ],
            ModelErrorKind::GenerationDegenerated,
        ),
        (
            "degeneration then malformed",
            vec![
                degenerate_content_generation(),
                malformed_generation(),
                answer_generation("this generation must never be requested"),
            ],
            ModelErrorKind::MalformedToolProposal,
        ),
    ];
    for (label, scripts, terminal_kind) in orderings {
        let model = fake_model(scripts);
        let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
        let audit = run(&model, &cancellation).await;

        assert_eq!(
            model.requests().len(),
            2,
            "{label}: the two classes share one corrective generation"
        );
        assert_eq!(
            corrective_requests(&model),
            vec![1],
            "{label}: exactly one corrective generation is authorized"
        );
        let terminal = assert_single_terminal(&audit.event_history);
        let RuntimeEvent::AttemptFailed {
            error: AttemptFailure::Model { error },
            ..
        } = terminal
        else {
            panic!("{label}: expected an explicit model-generation failure, got {terminal:?}");
        };
        assert_eq!(error.kind, terminal_kind, "{label}");
        assert!(assistant_texts(&audit).is_empty(), "{label}");
        assert_nothing_executed(&audit);
    }
}

/// Transport recovery is not semantic recovery. A transient provider failure
/// inside the corrective generation produces another *actual request* of the
/// same semantic generation: it neither restores nor duplicates the semantic
/// budget, and it carries the identical corrective hint.
///
/// ```text
/// #0  degenerate generation            -> corrective budget spent
/// #1  corrective generation, request A -> rate limit, no generation at all
/// #2  corrective generation, request B -> ordinary answer
/// ```
#[tokio::test]
async fn a_transient_retry_does_not_create_a_second_corrective_generation() {
    let model = fake_model(vec![
        degenerate_content_generation(),
        transient_generation(),
        answer_generation("answered after the retry"),
        answer_generation("this generation must never be requested"),
    ]);
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let audit = run(&model, &cancellation).await;

    assert_eq!(
        model.requests().len(),
        3,
        "three actual requests realize two semantic generations"
    );
    assert_eq!(
        corrective_requests(&model),
        vec![1, 2],
        "every actual request of the corrective generation carries the hint"
    );
    assert_eq!(assistant_texts(&audit), vec!["answered after the retry"]);
    assert!(!canonical_text(&audit).contains(LOOP_UNIT));
    assert_outcome(
        &audit,
        &AttemptOutcome::Completed {
            finish_reason: ModelFinishReason::Stop,
        },
    );
}

/// The other direction of the same invariant: a transient failure between two
/// semantic anomalies does not restore the spent corrective budget.
///
/// ```text
/// #0  degenerate generation            -> corrective budget spent
/// #1  corrective generation, request A -> rate limit
/// #2  corrective generation, request B -> degenerates again -> terminal
/// ```
#[tokio::test]
async fn a_transient_retry_does_not_restore_the_semantic_budget() {
    let model = fake_model(vec![
        degenerate_content_generation(),
        transient_generation(),
        degenerate_content_generation(),
        answer_generation("this generation must never be requested"),
    ]);
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let audit = run(&model, &cancellation).await;

    assert_eq!(model.requests().len(), 3);
    let terminal = assert_single_terminal(&audit.event_history);
    let RuntimeEvent::AttemptFailed {
        error: AttemptFailure::Model { error },
        ..
    } = terminal
    else {
        panic!("expected an explicit model-generation failure, got {terminal:?}");
    };
    assert_eq!(error.kind, ModelErrorKind::GenerationDegenerated);
    assert!(assistant_texts(&audit).is_empty());
    assert!(!canonical_text(&audit).contains(LOOP_UNIT));
}

/// The corrective hint is owned by its generation and disappears the moment
/// that generation resolves, so the tool→model continuation — a new semantic
/// generation — is never told to regenerate anything.
#[tokio::test]
async fn the_corrective_hint_does_not_survive_its_generation() {
    let call = ScriptedCall {
        id: "call-203-hint",
        tool_id: "tool-write",
        name: "write_file",
        arguments: serde_json::json!({"path": "notes.txt"}),
    };
    let model = fake_model(vec![
        degenerate_content_generation(),
        tool_generation(&call),
        answer_generation("written"),
    ]);
    let tool = FakeTool::new(write_tool_definition(), success_result("ok"));
    let mut tools = ToolRegistry::new();
    tool.register(&mut tools);
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let audit = run_with(&model, &cancellation, Setup::new().with_tools(tools)).await;

    assert_eq!(request_prompts(&model).len(), 3);
    assert_eq!(
        corrective_requests(&model),
        vec![1],
        "only the corrective generation carries the hint"
    );
    assert_eq!(tool_messages(&audit).len(), 1);
    assert_outcome(
        &audit,
        &AttemptOutcome::Completed {
            finish_reason: ModelFinishReason::Stop,
        },
    );
}

/// A timeout is transport liveness, not generation safety, so it consumes
/// the transient request-retry budget and leaves the semantic
/// corrective-generation budget untouched.
///
/// ```text
/// #0  timeout               -> transient retry, semantic budget still unused
/// #1  degenerate generation -> semantic budget consumed
/// #2  valid answer          -> commits
/// ```
///
/// If the timeout had consumed the shared semantic budget — the exact
/// regression the liveness/generation type split exists to prevent — the
/// degeneration at `#1` would have found it spent and terminated the attempt
/// instead of regenerating.
#[tokio::test]
async fn a_timeout_does_not_consume_the_semantic_corrective_budget() {
    let model = fake_model(vec![
        provider_timeout_generation(),
        degenerate_content_generation(),
        answer_generation("answered after the timeout and the loop"),
        answer_generation("this generation must never be requested"),
    ]);
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let audit = run(&model, &cancellation).await;

    assert_eq!(
        model.requests().len(),
        3,
        "one transport retry plus one semantic corrective generation"
    );
    assert_eq!(
        corrective_requests(&model),
        vec![2],
        "only the degeneration authorized a corrective generation; the timeout did not"
    );

    // The timeout is recorded as a liveness fact and carries no generation
    // detail; the degeneration is recorded as a generation fact and carries
    // no timeout phase. Neither is inferred from a message.
    let failures = generation_failures(&audit);
    assert_eq!(failures.len(), 2);
    assert_eq!(failures[0].0, ModelErrorKind::Timeout);
    assert!(
        failures[0].1.generation.is_none(),
        "a transport timeout is not a generation-safety fact"
    );
    assert_eq!(failures[1].0, ModelErrorKind::GenerationDegenerated);
    assert!(
        failures[1].1.timeout_phase.is_none(),
        "a generation defect is not a liveness fact"
    );

    assert_eq!(
        assistant_texts(&audit),
        vec!["answered after the timeout and the loop"]
    );
    assert!(!canonical_text(&audit).contains(LOOP_UNIT));
    assert_outcome(
        &audit,
        &AttemptOutcome::Completed {
            finish_reason: ModelFinishReason::Stop,
        },
    );
}

// ---------------------------------------------------------------------------
// Cancellation
// ---------------------------------------------------------------------------

/// Cancellation racing a rejected generation has exactly one terminal winner.
///
/// The interleaving is proven, not made likely: the loop parks at the exact
/// cut where the degenerate generation is durably settled and the corrective
/// generation has not yet been authorized. Cancellation is requested while
/// the loop is held there, so the recovery decision observes it. No second
/// provider request may start, and nothing from the rejected generation may
/// commit.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancellation_racing_a_rejected_generation_has_one_terminal_winner() {
    let model = fake_model(vec![
        degenerate_content_generation(),
        // Available on purpose: if cancellation lost the race, the corrective
        // generation would succeed and the request count would say so.
        answer_generation("this generation must never be requested"),
    ]);
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let tool_runtime = common::tool_runtime(CONVERSATION);
    let store = tool_runtime.durable_store();
    let capability = common::capability_lease(ToolRegistry::new(), &tool_runtime).await;
    let (pause, mut reached, release) =
        crate::agent::execution::test_sync::GenerationAnomalyPause::install();
    let mut execution = AgentExecution::new(
        request(&model),
        capability.into_lease(),
        &cancellation,
        support::default_execution_policy(),
        runtime(&model),
        &tool_runtime,
        rustx::agent::AttemptLifecycle::inert(),
    )
    .expect("conversation identity matches the tool runtime");
    execution.install_generation_anomaly_pause(pause);

    let controller_cancellation = cancellation.clone();
    let controller = tokio::spawn(async move {
        reached
            .wait_for(|parks| *parks >= 1)
            .await
            .expect("the generation anomaly pause remains open");
        // The rejected generation is durably settled and no corrective
        // generation has been authorized. Cancellation requested here
        // provably linearizes before the recovery decision.
        controller_cancellation.cancel();
        release.send(()).expect("release the recovery decision");
    });

    let audit = tokio::time::timeout(Duration::from_secs(5), async {
        common::durable_agent_result(execution.run().await, store.as_ref())
    })
    .await
    .expect("the cancellation race must settle without wall-clock waiting");
    controller.await.expect("the race controller completes");

    assert_eq!(
        model.requests().len(),
        1,
        "cancellation observable before the recovery decision starts no further request"
    );
    let terminal = assert_single_terminal(&audit.event_history);
    assert!(
        matches!(terminal, RuntimeEvent::AttemptCancelled { .. }),
        "cancellation is the single terminal winner, got {terminal:?}"
    );
    assert_outcome(
        &audit,
        &AttemptOutcome::Cancelled {
            reason: CancellationReason::UserRequested,
        },
    );
    assert!(
        assistant_texts(&audit).is_empty(),
        "the rejected generation commits nothing on either path"
    );
    assert!(!canonical_text(&audit).contains(LOOP_UNIT));
    assert_nothing_executed(&audit);
}
