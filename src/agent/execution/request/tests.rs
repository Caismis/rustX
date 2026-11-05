use crate::agent::cancellation::AgentCancellation;
use crate::agent::execution::AgentExecution;
use crate::agent::execution::test_fixtures::*;
use crate::context::ContextRuntime;
use crate::conversation::ConversationState;
use crate::durable::JournalStore;
use crate::durable::RequestStore;
use crate::events::types::AttemptOutcome;
use crate::message::types::{MessageBlock, UserContentBlock, UserSource};
use crate::model::event::ModelEvent;
use crate::model::finish::ModelFinishReason;
use crate::runtime::identity::{AttemptId, ConversationId};
use crate::runtime::types::CancellationReason;
use crate::scripted_suites::common::{capability_lease, tool_runtime, tool_runtime_with_store};
use crate::scripted_suites::support::fake::inbound_message;
use crate::tools::executor::ToolRegistry;
use futures_util::future::BoxFuture;
use std::sync::Arc;
use tokio::sync::watch;

struct CountingContext {
    invocations: Arc<std::sync::atomic::AtomicUsize>,
    add_message: bool,
}

impl crate::context::ContextContributor for CountingContext {
    fn contribute<'a>(
        &'a self,
        _input: &'a crate::context::ContributorInputSnapshot,
    ) -> BoxFuture<
        'a,
        Result<Vec<crate::context::ContextProposal>, crate::context::ContextAssemblyError>,
    > {
        self.invocations
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Box::pin(async move {
            Ok(if self.add_message {
                vec![crate::context::ContextProposal::UserMessage(
                    crate::context::UserMessageProposal {
                        content: vec![UserContentBlock::Text(crate::message::content::TextBlock {
                            text: "accepted staged context".to_owned(),
                        })],
                    },
                )]
            } else {
                Vec::new()
            })
        })
    }
}

async fn failed_summary_attempt(
    historical_messages: usize,
    staged_message: bool,
    hard_input_limit: u64,
) -> (
    crate::scripted_suites::common::DurableExecutionAudit,
    Arc<ScriptedAdapter>,
    Arc<crate::scripted_suites::support::context::FakeContextSummarizer>,
    Arc<std::sync::atomic::AtomicUsize>,
) {
    failed_summary_attempt_with_kind(
        historical_messages,
        staged_message,
        hard_input_limit,
        crate::context::ContextErrorKind::SummaryFailed,
    )
    .await
}

async fn failed_summary_attempt_with_kind(
    historical_messages: usize,
    staged_message: bool,
    hard_input_limit: u64,
    failure_kind: crate::context::ContextErrorKind,
) -> (
    crate::scripted_suites::common::DurableExecutionAudit,
    Arc<ScriptedAdapter>,
    Arc<crate::scripted_suites::support::context::FakeContextSummarizer>,
    Arc<std::sync::atomic::AtomicUsize>,
) {
    failed_summary_attempt_with_feedback(
        historical_messages,
        staged_message,
        hard_input_limit,
        failure_kind,
        None,
        false,
    )
    .await
}

async fn failed_summary_attempt_with_feedback(
    historical_messages: usize,
    staged_message: bool,
    hard_input_limit: u64,
    failure_kind: crate::context::ContextErrorKind,
    feedback: Option<String>,
    successful_summary: bool,
) -> (
    crate::scripted_suites::common::DurableExecutionAudit,
    Arc<ScriptedAdapter>,
    Arc<crate::scripted_suites::support::context::FakeContextSummarizer>,
    Arc<std::sync::atomic::AtomicUsize>,
) {
    use crate::context::{CompactionBudgets, ContextConfig, ContextEngine, ContextError};
    use crate::scripted_suites::support::context::{
        FakeContextSummarizer, FakeSummaryStep, ScriptedEstimator,
    };

    let adapter = Arc::new(ScriptedAdapter::new(vec![workflow_output_turn(
        &[],
        Some("answer"),
    )]));
    let failure = ContextError::new(failure_kind, "summary service unavailable");
    let summarizer = Arc::new(FakeContextSummarizer::new(if successful_summary {
        vec![FakeSummaryStep::Return("short summary".into())]
    } else {
        vec![
            FakeSummaryStep::Fail(failure.clone()),
            FakeSummaryStep::Fail(failure),
        ]
    }));
    let invocations = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut assembly = crate::context::ContextAssembly::new();
    assembly
        .register_extension(
            "counting.test",
            Some("generation-1".to_owned()),
            Arc::new(CountingContext {
                invocations: invocations.clone(),
                add_message: staged_message,
            }),
        )
        .unwrap();
    let engine = ContextEngine::new(
        ContextConfig {
            context_window_tokens: hard_input_limit + 1,
            reserve_tokens: 0,
            keep_recent_tokens: 0,
        },
        Arc::new(ScriptedEstimator::new(50, 10, 0)),
    )
    .unwrap();
    let runtime = ContextRuntime::with_scripted_summarizer_and_assembly(
        engine,
        summarizer.clone(),
        None,
        assembly,
        CompactionBudgets::new(1, 1, 1_000_000),
    );
    let mut request = request(&adapter);
    request.model = crate::scripted_suites::support::attempt_model_with_window(
        adapter.clone(),
        "compaction-test",
        hard_input_limit + 1,
        1,
    );
    request.conversation = ConversationState::from_messages(
        (0..historical_messages)
            .map(|index| {
                MessageBlock::User(inbound_message(
                    &format!("historical-{index}"),
                    "old history",
                    UserSource::Human,
                ))
            })
            .collect::<Vec<_>>(),
    )
    .unwrap();
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let tool_runtime = tool_runtime("conv_36524fd8-f674-7fc2-8125-06d01fee0e18");
    let lease = capability_lease(ToolRegistry::new(), &tool_runtime)
        .await
        .into_lease();
    let mut execution = AgentExecution::new(
        request,
        lease,
        &cancellation,
        crate::scripted_suites::support::default_execution_policy(),
        runtime,
        &tool_runtime,
        crate::agent::AttemptLifecycle::inert(),
    )
    .unwrap();
    execution.workflow_output_feedback = feedback;
    let result = execution.run().await;
    let audit = crate::scripted_suites::common::durable_agent_result(
        result,
        tool_runtime.durable_store().as_ref(),
    );
    (audit, adapter, summarizer, invocations)
}

const PROVENANCE_CEILING_DIAGNOSTIC: &str =
    "compaction provenance exceeds inline budget before summary invocation";

struct ProvenanceCeilingRun {
    outcome: AttemptOutcome,
    requests: Vec<crate::model::ModelRequest>,
    summary_calls: usize,
    events: Vec<crate::events::types::RuntimeEvent>,
    hard_input_limit: u64,
    has_projection: bool,
}

async fn provenance_ceiling_attempt(mandatory: bool) -> ProvenanceCeilingRun {
    use crate::context::{
        CompactionBudgets, ContextConfig, ContextEngine, DefaultTokenEstimator, TokenEstimator,
    };
    use crate::runtime::identity::{ToolCallId, ToolId};
    use crate::scripted_suites::support::context::FakeContextSummarizer;
    use crate::tools::types::ToolCall;

    let call = ToolCall {
        id: ToolCallId::new("provenance-call"),
        tool_id: ToolId::new("tool-alpha"),
        name: "alpha".to_owned(),
        arguments: serde_json::json!({}),
    };
    let adapter = Arc::new(ScriptedAdapter::new(vec![
        tool_call_script(&call),
        workflow_output_turn(&[], Some("answer")),
    ]));
    let mut tools = ToolRegistry::new();
    tools
        .register(
            InstantTool::definition("tool-alpha", "alpha"),
            Arc::new(InstantTool),
        )
        .unwrap();
    let history: Vec<_> = (0..1_500)
        .map(|index| {
            MessageBlock::User(inbound_message(
                &format!("provenance-history-{index}"),
                "old history",
                UserSource::Human,
            ))
        })
        .collect();
    let selected: Vec<_> = history.iter().cloned().map(Into::into).collect();
    let estimated_input =
        DefaultTokenEstimator.estimate_input(&selected, "", &tools.model_definitions());
    // The optional case has headroom for the tool continuation while both
    // requests remain above the 90% proactive threshold. The mandatory case
    // exceeds the hard limit before any primary start.
    let hard_input_limit = if mandatory {
        estimated_input - 1
    } else {
        estimated_input + estimated_input / 20
    };
    let engine = ContextEngine::new(
        ContextConfig {
            context_window_tokens: hard_input_limit + 1,
            reserve_tokens: 0,
            keep_recent_tokens: 0,
        },
        Arc::new(DefaultTokenEstimator),
    )
    .unwrap();
    let summarizer = Arc::new(FakeContextSummarizer::new(Vec::new()));
    let runtime = ContextRuntime::with_scripted_summarizer(
        engine,
        summarizer.clone(),
        None,
        CompactionBudgets::new(1, 1, 1_000_000),
    );
    let mut request = request(&adapter);
    request.model = crate::scripted_suites::support::attempt_model_with_window(
        adapter.clone(),
        "provenance-ceiling-test",
        hard_input_limit + 1,
        1,
    );
    request.conversation = ConversationState::from_messages(history).unwrap();
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let tool_runtime = tool_runtime("conv_36524fd8-f674-7fc2-8125-06d01fee0e18");
    let lease = capability_lease(tools, &tool_runtime).await.into_lease();
    let result = AgentExecution::new(
        request,
        lease,
        &cancellation,
        crate::scripted_suites::support::default_execution_policy(),
        runtime,
        &tool_runtime,
        crate::agent::AttemptLifecycle::inert(),
    )
    .unwrap()
    .run()
    .await;
    let store = tool_runtime.durable_store();
    ProvenanceCeilingRun {
        outcome: result.outcome,
        requests: adapter.requests(),
        summary_calls: summarizer.requests().len(),
        events: event_history(store.as_ref()),
        hard_input_limit,
        has_projection: store.projection_head().unwrap().is_some(),
    }
}

#[tokio::test]
async fn optional_provenance_ceiling_defers_and_suppresses_retries_for_the_attempt() {
    use crate::context::{DefaultTokenEstimator, TokenEstimator};
    use crate::events::types::RuntimeEvent;

    let run = provenance_ceiling_attempt(false).await;
    assert!(matches!(run.outcome, AttemptOutcome::Completed { .. }));
    assert_eq!(run.requests.len(), 2);
    assert_eq!(run.summary_calls, 0);
    assert!(!run.has_projection);
    for request in &run.requests {
        let estimate = DefaultTokenEstimator.estimate_frozen_request(request);
        assert!(estimate >= run.hard_input_limit * 9 / 10);
        assert!(estimate <= run.hard_input_limit);
    }
    assert_eq!(
        run.events
            .iter()
            .filter(|event| matches!(event, RuntimeEvent::CompactionStarted { .. }))
            .count(),
        1,
        "the second pressured step must not retry optional compaction"
    );
    let failure = run
        .events
        .iter()
        .position(|event| matches!(event, RuntimeEvent::CompactionFailed { error, .. } if error == PROVENANCE_CEILING_DIAGNOSTIC))
        .expect("the real provenance ceiling was reported");
    let starts: Vec<_> = run
        .events
        .iter()
        .enumerate()
        .filter_map(|(index, event)| {
            matches!(
                event,
                RuntimeEvent::ModelRequestStarted {
                    owner: crate::model::ModelRequestOwner::Agent { .. },
                    ..
                }
            )
            .then_some(index)
        })
        .collect();
    assert_eq!(starts.len(), 2);
    assert!(
        failure < starts[0],
        "both requests use the normal start gate after deferral"
    );
    assert!(
        !run.events
            .iter()
            .any(|event| matches!(event, RuntimeEvent::CompactionCompleted { .. }))
    );
}

#[tokio::test]
async fn mandatory_provenance_ceiling_fails_without_starting_the_primary() {
    use crate::events::types::{AttemptFailure, RuntimeEvent};
    use crate::runtime::types::RuntimeError;

    let run = provenance_ceiling_attempt(true).await;
    assert!(matches!(
        run.outcome,
        AttemptOutcome::Failed {
            error: AttemptFailure::Runtime {
                error: RuntimeError::ContextCompactionFailed { message }
            }
        } if message == PROVENANCE_CEILING_DIAGNOSTIC
    ));
    assert!(run.requests.is_empty());
    assert_eq!(run.summary_calls, 0);
    assert!(!run.has_projection);
    assert!(run.events.iter().any(|event| matches!(
        event,
        RuntimeEvent::CompactionFailed { error, .. } if error == PROVENANCE_CEILING_DIAGNOSTIC
    )));
    assert!(
        !run.events
            .iter()
            .any(|event| matches!(event, RuntimeEvent::ModelRequestStarted { .. }))
    );
}

#[tokio::test]
async fn optional_summary_failure_reuses_the_once_accepted_staged_context() {
    use std::sync::atomic::Ordering;
    // Two historical messages make the request proactively full while still
    // leaving room for the minimum structured Summary and staged context.
    let (result, adapter, summarizer, invocations) = failed_summary_attempt(2, true, 160).await;
    assert!(matches!(result.outcome, AttemptOutcome::Completed { .. }));
    assert_eq!(adapter.request_count(), 1);
    assert_eq!(summarizer.requests().len(), 1);
    assert_eq!(invocations.load(Ordering::SeqCst), 1);
    assert!(result.projections.is_empty());
    assert!(result.event_history.iter().any(|event| matches!(
        event,
        crate::events::types::RuntimeEvent::CompactionFailed { .. }
    )));
    assert!(adapter.requests()[0].messages.iter().any(|message| {
        message.as_canonical().is_some_and(|message| match message {
            MessageBlock::User(user) => user.content.iter().any(|part| matches!(part, UserContentBlock::Text(text) if text.text == "accepted staged context")),
            _ => false,
        })
    }));
}

#[tokio::test]
async fn summary_failure_cannot_bypass_mandatory_primary_admission() {
    let (result, adapter, _, invocations) = failed_summary_attempt(3, false, 110).await;
    assert!(matches!(result.outcome, AttemptOutcome::Failed { .. }));
    assert_eq!(adapter.request_count(), 0);
    assert_eq!(invocations.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert!(result.projections.is_empty());
    assert!(result.event_history.iter().any(|event| matches!(
        event,
        crate::events::types::RuntimeEvent::CompactionFailed { .. }
    )));
}

#[tokio::test]
async fn complete_staged_pressure_is_mandatory_before_the_first_compaction() {
    use std::sync::atomic::Ordering;
    let (result, adapter, _, invocations) = failed_summary_attempt(2, true, 110).await;
    assert!(matches!(result.outcome, AttemptOutcome::Failed { .. }));
    assert_eq!(adapter.request_count(), 0);
    assert_eq!(invocations.load(Ordering::SeqCst), 1);
    assert!(result.projections.is_empty());
    assert_eq!(
        result
            .event_history
            .iter()
            .filter(|event| matches!(
                event,
                crate::events::types::RuntimeEvent::CompactionStarted { .. }
            ))
            .count(),
        1
    );
}

#[tokio::test]
async fn a_failed_optional_summary_does_not_reject_exact_hard_limit_equality() {
    let (result, adapter, summarizer, _) = failed_summary_attempt(2, false, 100).await;
    assert!(matches!(result.outcome, AttemptOutcome::Completed { .. }));
    assert_eq!(adapter.request_count(), 1);
    assert_eq!(summarizer.requests().len(), 1);
    assert!(result.projections.is_empty());
}

#[tokio::test]
async fn optional_compaction_cannot_continue_after_cancellation_or_durable_failure() {
    use crate::context::ContextErrorKind;
    for kind in [
        ContextErrorKind::Cancelled,
        ContextErrorKind::AuxiliaryDurabilityFailed,
    ] {
        let (result, adapter, summarizer, _) =
            failed_summary_attempt_with_kind(2, true, 160, kind).await;
        assert_eq!(summarizer.requests().len(), 1, "{kind:?}");
        assert_eq!(adapter.request_count(), 0, "{kind:?}");
        assert!(result.projections.is_empty());
        if kind == ContextErrorKind::Cancelled {
            assert!(matches!(result.outcome, AttemptOutcome::Cancelled { .. }));
            assert!(!result.event_history.iter().any(|event| matches!(
                event,
                crate::events::types::RuntimeEvent::CompactionFailed { .. }
            )));
            assert_eq!(
                result
                    .event_history
                    .iter()
                    .filter(|event| matches!(
                        event,
                        crate::events::types::RuntimeEvent::CompactionCancelled { .. }
                    ))
                    .count(),
                1
            );
            assert!(matches!(
                result.event_history.last(),
                Some(crate::events::types::RuntimeEvent::AttemptCancelled { .. })
            ));
        } else {
            assert!(matches!(result.outcome, AttemptOutcome::Failed { .. }));
        }
    }
}

#[tokio::test]
async fn optional_planning_failure_and_exhausted_summary_can_preserve_a_fitting_request() {
    use crate::context::ContextErrorKind;
    for kind in [
        ContextErrorKind::SummaryOutputExhausted,
        ContextErrorKind::CannotFit,
        ContextErrorKind::NoProgress,
        ContextErrorKind::SearchIncomplete,
    ] {
        let (result, adapter, summarizer, _) =
            failed_summary_attempt_with_kind(2, true, 160, kind).await;
        assert_eq!(summarizer.requests().len(), 1, "{kind:?}");
        assert!(
            matches!(result.outcome, AttemptOutcome::Completed { .. }),
            "{kind:?}"
        );
        assert_eq!(adapter.request_count(), 1, "{kind:?}");
        assert!(result.projections.is_empty());
    }
}

#[tokio::test]
async fn proactive_pressure_without_eligible_history_keeps_the_fitting_staged_request() {
    let (result, adapter, summarizer, invocations) = failed_summary_attempt(0, true, 50).await;
    assert!(matches!(result.outcome, AttemptOutcome::Completed { .. }));
    assert_eq!(adapter.request_count(), 1);
    assert!(summarizer.requests().is_empty());
    assert_eq!(invocations.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert!(result.projections.is_empty());
    assert!(result.event_history.iter().any(|event| matches!(
        event,
        crate::events::types::RuntimeEvent::CompactionSkipped { .. }
    )));
    assert!(!result.event_history.iter().any(|event| matches!(
        event,
        crate::events::types::RuntimeEvent::CompactionFailed { .. }
    )));
}

#[tokio::test]
async fn proactive_pressure_prices_frozen_request_only_feedback() {
    let feedback = "x".repeat(168);
    let (result, adapter, summarizer, invocations) = failed_summary_attempt_with_feedback(
        2,
        false,
        160,
        crate::context::ContextErrorKind::SummaryFailed,
        Some(feedback.clone()),
        false,
    )
    .await;
    assert!(matches!(result.outcome, AttemptOutcome::Completed { .. }));
    assert_eq!(adapter.request_count(), 1);
    assert_eq!(summarizer.requests().len(), 1);
    assert_eq!(invocations.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert!(
        adapter.requests()[0]
            .effective_system_prompt
            .contains(&feedback)
    );
    let (without_feedback, baseline_adapter, baseline_summarizer, _) =
        failed_summary_attempt_with_feedback(
            2,
            false,
            160,
            crate::context::ContextErrorKind::SummaryFailed,
            None,
            false,
        )
        .await;
    assert!(matches!(
        without_feedback.outcome,
        AttemptOutcome::Completed { .. }
    ));
    assert_eq!(baseline_adapter.request_count(), 1);
    assert!(baseline_summarizer.requests().is_empty());
}

#[tokio::test]
async fn fixed_request_only_feedback_cannot_activate_an_inadmissible_checkpoint() {
    let (result, adapter, summarizer, invocations) = failed_summary_attempt_with_feedback(
        2,
        false,
        110,
        crate::context::ContextErrorKind::SummaryFailed,
        Some("x".repeat(512)),
        true,
    )
    .await;
    assert!(matches!(result.outcome, AttemptOutcome::Failed { .. }));
    assert_eq!(adapter.request_count(), 0);
    assert!(summarizer.requests().is_empty());
    assert_eq!(invocations.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert!(
        result.projections.is_empty(),
        "fixed input must be priced before activation"
    );
}

#[tokio::test]
async fn workflow_output_tool_is_priced_before_checkpoint_activation() {
    use crate::context::{CompactionBudgets, ContextConfig, ContextEngine};
    use crate::scripted_suites::support::context::{
        FakeContextSummarizer, FakeSummaryStep, ScriptedEstimator,
    };

    let adapter = Arc::new(ScriptedAdapter::new(vec![workflow_output_turn(
        &[],
        Some("answer"),
    )]));
    let summarizer = Arc::new(FakeContextSummarizer::new(vec![FakeSummaryStep::Return(
        "short summary".into(),
    )]));
    let engine = ContextEngine::new(
        ContextConfig {
            context_window_tokens: 111,
            reserve_tokens: 0,
            keep_recent_tokens: 0,
        },
        Arc::new(ScriptedEstimator::new(50, 10, 120)),
    )
    .unwrap();
    let runtime = ContextRuntime::with_scripted_summarizer(
        engine,
        summarizer.clone(),
        None,
        CompactionBudgets::new(1, 1, 1_000_000),
    );
    let mut request = request(&adapter);
    request.model = crate::scripted_suites::support::attempt_model_with_window(
        adapter.clone(),
        "compaction-test",
        111,
        1,
    );
    request.conversation = ConversationState::from_messages(vec![MessageBlock::User(
        inbound_message("workflow-history", "old history", UserSource::Human),
    )])
    .unwrap();
    let latch = Arc::new(
        crate::runtime::workflow::WorkflowOutputLatch::new(serde_json::json!({
            "type": "object", "properties": {"ok": {"type": "boolean"}},
            "required": ["ok"], "additionalProperties": false
        }))
        .unwrap(),
    );
    let mut policy = crate::scripted_suites::support::default_execution_policy();
    policy.workflow_output = Some(latch);
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let tool_runtime = tool_runtime("conv_36524fd8-f674-7fc2-8125-06d01fee0e18");
    let lease = capability_lease(ToolRegistry::new(), &tool_runtime)
        .await
        .into_lease();
    let result = AgentExecution::new(
        request,
        lease,
        &cancellation,
        policy,
        runtime,
        &tool_runtime,
        crate::agent::AttemptLifecycle::inert(),
    )
    .unwrap()
    .run()
    .await;
    assert!(matches!(result.outcome, AttemptOutcome::Failed { .. }));
    assert_eq!(adapter.request_count(), 0);
    assert!(summarizer.requests().is_empty());
    assert!(
        tool_runtime
            .durable_store()
            .projection_head()
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn overflow_recovery_keeps_request_only_feedback_in_the_compaction_fit() {
    use crate::context::{CompactionBudgets, ContextConfig, ContextEngine};
    use crate::model::{ModelError, ModelErrorKind, ModelRetryDisposition};
    use crate::scripted_suites::support::context::{
        FakeContextSummarizer, FakeSummaryStep, ScriptedEstimator,
    };

    let adapter = Arc::new(ScriptedAdapter::new(vec![vec![
        ModelEvent::Started,
        ModelEvent::Failed {
            error: ModelError {
                kind: ModelErrorKind::ContextWindowExceeded,
                message: "provider rejected input".into(),
                retry_disposition: ModelRetryDisposition::Never,
                retry_after_ms: None,
                provider_code: None,
                context_overflow: None,
                malformed_tool_proposal: None,
                timeout_phase: None,
                generation: None,
            },
        },
    ]]));
    let summarizer = Arc::new(FakeContextSummarizer::new(vec![FakeSummaryStep::Return(
        "short summary".into(),
    )]));
    let engine = ContextEngine::new(
        ContextConfig {
            context_window_tokens: 201,
            reserve_tokens: 0,
            keep_recent_tokens: 0,
        },
        Arc::new(ScriptedEstimator::new(10, 1, 0)),
    )
    .unwrap();
    let runtime = ContextRuntime::with_scripted_summarizer(
        engine,
        summarizer.clone(),
        None,
        CompactionBudgets::new(1, 1, 1_000_000),
    );
    let mut request = request(&adapter);
    request.model = crate::scripted_suites::support::attempt_model_with_window(
        adapter.clone(),
        "compaction-test",
        201,
        1,
    );
    request.conversation = ConversationState::from_messages(vec![MessageBlock::User(
        inbound_message("overflow-history", "old history", UserSource::Human),
    )])
    .unwrap();
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let tool_runtime = tool_runtime("conv_36524fd8-f674-7fc2-8125-06d01fee0e18");
    let lease = capability_lease(ToolRegistry::new(), &tool_runtime)
        .await
        .into_lease();
    let mut execution = AgentExecution::new(
        request,
        lease,
        &cancellation,
        crate::scripted_suites::support::default_execution_policy(),
        runtime,
        &tool_runtime,
        crate::agent::AttemptLifecycle::inert(),
    )
    .unwrap();
    execution.workflow_output_feedback = Some("x".repeat(600));
    let result = execution.run().await;
    assert!(matches!(result.outcome, AttemptOutcome::Failed { .. }));
    assert_eq!(
        adapter.request_count(),
        1,
        "the rejected primary request is not replayed"
    );
    assert!(
        summarizer.requests().is_empty(),
        "fixed feedback cannot fit the corrected budget"
    );
    assert!(
        tool_runtime
            .durable_store()
            .projection_head()
            .unwrap()
            .is_none()
    );
}

/// A contributor whose bounded work is explicitly held at an awaited
/// boundary. The test releases it only after cancellation is observable,
/// proving that the final admission check—not the contributor's private
/// synchronization—decides whether transient proposals become facts.
struct AwaitingContributor {
    entered: watch::Sender<bool>,
    release: watch::Receiver<bool>,
    invocations: Arc<std::sync::atomic::AtomicUsize>,
}

impl crate::context::ContextContributor for AwaitingContributor {
    fn contribute<'a>(
        &'a self,
        _input: &'a crate::context::ContributorInputSnapshot,
    ) -> BoxFuture<
        'a,
        Result<Vec<crate::context::ContextProposal>, crate::context::ContextAssemblyError>,
    > {
        self.invocations
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.entered.send_replace(true);
        let mut release = self.release.clone();
        Box::pin(async move {
            release
                .wait_for(|released| *released)
                .await
                .expect("contributor release channel stays open");
            Ok(vec![crate::context::ContextProposal::UserMessage(
                crate::context::UserMessageProposal {
                    content: vec![UserContentBlock::Text(crate::message::content::TextBlock {
                        text: "awaited proposal".to_owned(),
                    })],
                },
            )])
        })
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[allow(clippy::too_many_lines)] // One contributor gate proves ordering across assembly, compaction and activation.
async fn complete_context_assembly_precedes_any_compaction_activation() {
    use crate::context::{
        CompactionBudgets, ContextConfig, ContextEngine, ContextError, ContextErrorKind,
    };
    use crate::events::types::RuntimeEvent;
    use crate::scripted_suites::support::context::{
        FakeContextSummarizer, FakeSummaryStep, ScriptedEstimator,
    };

    let adapter = Arc::new(ScriptedAdapter::new(vec![workflow_output_turn(
        &[],
        Some("answer"),
    )]));
    let (entered, mut entered_rx) = watch::channel(false);
    let (release, release_rx) = watch::channel(false);
    let invocations = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut assembly = crate::context::ContextAssembly::new();
    assembly
        .register_extension(
            "awaited.test",
            Some("generation-1".into()),
            Arc::new(AwaitingContributor {
                entered,
                release: release_rx,
                invocations: invocations.clone(),
            }),
        )
        .unwrap();
    let engine = ContextEngine::new(
        ContextConfig {
            context_window_tokens: 111,
            reserve_tokens: 0,
            keep_recent_tokens: 0,
        },
        Arc::new(ScriptedEstimator::new(50, 10, 0)),
    )
    .unwrap();
    let summarizer = Arc::new(FakeContextSummarizer::new(vec![FakeSummaryStep::Fail(
        ContextError::new(
            ContextErrorKind::SummaryFailed,
            "summary service unavailable",
        ),
    )]));
    let runtime = ContextRuntime::with_scripted_summarizer_and_assembly(
        engine,
        summarizer,
        None,
        assembly,
        CompactionBudgets::new(1, 1, 1_000_000),
    );
    let mut request = request(&adapter);
    request.model = crate::scripted_suites::support::attempt_model_with_window(
        adapter.clone(),
        "compaction-test",
        111,
        1,
    );
    request.conversation = ConversationState::from_messages(
        (0..3)
            .map(|index| {
                MessageBlock::User(inbound_message(
                    &format!("pre-assembly-{index}"),
                    "old history",
                    UserSource::Human,
                ))
            })
            .collect::<Vec<_>>(),
    )
    .unwrap();
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let tool_runtime = tool_runtime("conv_36524fd8-f674-7fc2-8125-06d01fee0e18");
    let lease = capability_lease(ToolRegistry::new(), &tool_runtime)
        .await
        .into_lease();
    let execution = AgentExecution::new(
        request,
        lease,
        &cancellation,
        crate::scripted_suites::support::default_execution_policy(),
        runtime,
        &tool_runtime,
        crate::agent::AttemptLifecycle::inert(),
    )
    .unwrap();
    let store = tool_runtime.durable_store();
    let monitor = async {
        entered_rx.wait_for(|entered| *entered).await.unwrap();
        let events = store.read_events(None, 32).unwrap().events;
        assert!(
            !events
                .iter()
                .any(|event| matches!(event.event, RuntimeEvent::CompactionStarted { .. }))
        );
        assert!(store.projection_head().unwrap().is_none());
        release.send_replace(true);
    };
    let (result, ()) = tokio::join!(execution.run(), monitor);
    assert!(matches!(result.outcome, AttemptOutcome::Failed { .. }));
    assert_eq!(invocations.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert!(
        store
            .read_events(None, 32)
            .unwrap()
            .events
            .iter()
            .any(|event| matches!(event.event, RuntimeEvent::CompactionStarted { .. }))
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[allow(
    clippy::too_many_lines,
    reason = "exercise native Summary settlement and live observation in one ordered scenario"
)]
async fn accepted_chunk_is_published_live_before_later_cancellation() {
    use crate::context::{CompactionBudgets, ContextConfig, ContextEngine};
    use crate::events::types::RuntimeEvent;
    use crate::message::types::ContentBlockIndex;
    use crate::scripted_suites::support::{
        context::ScriptedEstimator,
        fake::{FakeStep, fake_model},
    };

    let adapter = Arc::new(ScriptedAdapter::new(vec![workflow_output_turn(
        &[],
        Some("answer"),
    )]));
    let summary_json = r#"{"goal":"continue","constraints_and_preferences":[],"progress":{"done":[],"in_progress":[],"blocked":[]},"key_decisions":[],"next_steps":[],"critical_context":[]}"#;
    let summary_model = fake_model(vec![
        vec![
            FakeStep::Emit(ModelEvent::Started),
            FakeStep::Emit(ModelEvent::TextDelta {
                block_index: ContentBlockIndex::new(0),
                text: summary_json.into(),
            }),
            FakeStep::Emit(ModelEvent::Completed {
                finish_reason: ModelFinishReason::Stop,
                usage: None,
            }),
        ],
        vec![FakeStep::ParkUntilCancelled],
    ]);
    let mut parked = summary_model.parked();
    let summary_invocation = crate::scripted_suites::support::attempt_model_with_window(
        summary_model.clone(),
        "summary-test",
        10_000_000,
        1024,
    )
    .summary_invocation()
    .clone();
    let summarizer = Arc::new(crate::context::ModelBackedSummarizer::new(
        summary_invocation.clone(),
        crate::model::ModelTimeoutPolicy::default(),
        crate::scripted_suites::support::default_monotonic_clock(),
    ));
    let engine = ContextEngine::new(
        ContextConfig {
            context_window_tokens: 111,
            reserve_tokens: 0,
            keep_recent_tokens: 0,
        },
        Arc::new(ScriptedEstimator::new(50, 10, 0)),
    )
    .unwrap();
    let mut request = request(&adapter);
    request.model = crate::scripted_suites::support::attempt_model_with_window(
        adapter.clone(),
        "compaction-test",
        111,
        1,
    );
    request.conversation = ConversationState::from_messages(
        (0..2)
            .map(|index| {
                MessageBlock::User(inbound_message(
                    &format!("fragment-{index}"),
                    &"x".repeat(300),
                    UserSource::Human,
                ))
            })
            .collect::<Vec<_>>(),
    )
    .unwrap();
    // Price the complete two-source native request, including its structured
    // output contract, then make it just too large so a second Call is needed.
    let selected: crate::model::SelectedModelInput = request
        .conversation
        .ledger()
        .audit_records()
        .into_iter()
        .map(Into::into)
        .collect::<Vec<_>>()
        .into();
    let chunks = crate::context::chunks::ChunkPlan::new(&selected, 2).unwrap();
    let complete = chunks
        .next(None, None, Some(1024), u64::MAX, false, |_| Ok(0))
        .unwrap();
    let complete_request = crate::runtime::auxiliary::output::AuxiliaryOutputFormat::JsonSchema(
        crate::context::summary_content::response_schema(),
    )
    .prepare(complete.model_request(&summary_invocation))
    .unwrap();
    let summary_input_limit = engine.estimate_frozen_request(&complete_request) - 1;
    let runtime = ContextRuntime::with_scripted_summarizer(
        engine,
        summarizer,
        None,
        CompactionBudgets::new(1, 1024, summary_input_limit),
    );
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let tool_runtime = tool_runtime("conv_36524fd8-f674-7fc2-8125-06d01fee0e18");
    let lease = capability_lease(ToolRegistry::new(), &tool_runtime)
        .await
        .into_lease();
    let observer = RecordingObserver::default();
    let mut execution = AgentExecution::new(
        request,
        lease,
        &cancellation,
        crate::scripted_suites::support::default_execution_policy(),
        runtime,
        &tool_runtime,
        crate::agent::AttemptLifecycle::inert(),
    )
    .unwrap();
    execution.observe(&observer);
    let store = tool_runtime.durable_store();
    let mut run = Box::pin(execution.run());
    tokio::select! {
        result = &mut run => panic!(
            "attempt settled before the second Summary request: {:?}; events: {:?}",
            result.outcome,
            observer.events(),
        ),
        parked = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            parked.wait_for(|value| *value),
        ) => {
            parked.expect("second Summary request entered").unwrap();
        }
    }
    let events = observer.events();
    let chunk = events
        .iter()
        .find(|event| matches!(event, RuntimeEvent::CompactionChunkAccepted { .. }))
        .expect("the first accepted chunk was published before the second request settled");
    assert!(
        store
            .read_events(None, 32)
            .unwrap()
            .events
            .iter()
            .any(|event| &event.event == chunk)
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, RuntimeEvent::CompactionCompleted { .. }))
    );
    cancellation.cancel();
    let result = run.await;
    assert!(matches!(result.outcome, AttemptOutcome::Cancelled { .. }));
    let events = observer.events();
    assert!(
        events
            .iter()
            .any(|event| matches!(event, RuntimeEvent::CompactionCancelled { .. }))
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, RuntimeEvent::CompactionCompleted { .. }))
    );
    assert!(store.projection_head().unwrap().is_none());
}

/// Cancellation may become observable while a contributor is awaiting
/// bounded work. The contributor still settles its transient proposal,
/// then the one start arbitration linearizes cancellation before any
/// canonical context, Surface revision, snapshot, or provider request
/// exists (Issue #12, M9b).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancellation_during_awaited_context_assembly_commits_nothing() {
    let adapter = Arc::new(ScriptedAdapter::new(vec![vec![ModelEvent::Completed {
        finish_reason: ModelFinishReason::Stop,
        usage: None,
    }]]));
    let invocations = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let (entered, mut entered_rx) = watch::channel(false);
    let (release, release_rx) = watch::channel(false);
    let mut assembly = crate::context::ContextAssembly::new();
    assembly
        .register_extension(
            "awaited.test",
            Some("package-v1".to_owned()),
            Arc::new(AwaitingContributor {
                entered,
                release: release_rx,
                invocations: Arc::clone(&invocations),
            }),
        )
        .expect("register awaited contributor");
    let request = request(&adapter);
    let runtime = ContextRuntime::for_attempt_with_assembly(
        Arc::new(crate::context::DefaultTokenEstimator),
        Some(crate::context::AgentStatusEngine::default()),
        assembly,
        &request.model,
        crate::model::ModelTimeoutPolicy::default(),
        crate::scripted_suites::support::default_monotonic_clock(),
    )
    .expect("valid context runtime");
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let controller_cancellation = cancellation.clone();
    let controller = tokio::spawn(async move {
        entered_rx
            .wait_for(|entered| *entered)
            .await
            .expect("awaited contributor entered");
        controller_cancellation.cancel();
        release.send_replace(true);
    });
    let tool_runtime = tool_runtime("conv_36524fd8-f674-7fc2-8125-06d01fee0e18");
    let lease = capability_lease(ToolRegistry::new(), &tool_runtime)
        .await
        .into_lease();
    let result = AgentExecution::new(
        request,
        lease,
        &cancellation,
        crate::scripted_suites::support::default_execution_policy(),
        runtime,
        &tool_runtime,
        crate::agent::AttemptLifecycle::inert(),
    )
    .expect("conversation identity matches the tool runtime")
    .run()
    .await;
    controller.await.expect("cancellation controller");
    let snapshots = request_snapshot_history(tool_runtime.durable_store().as_ref());

    assert_eq!(
        invocations.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "the awaited contributor is invoked once"
    );
    assert!(matches!(
        result.outcome,
        crate::events::types::AttemptOutcome::Cancelled {
            reason: CancellationReason::UserRequested
        }
    ));
    assert!(snapshots.is_empty());
    assert_eq!(
        result.conversation.revision(),
        crate::conversation::SurfaceRevision::INITIAL
    );
    assert!(result.messages().is_empty());
    assert_eq!(adapter.request_count(), 0);
}

/// Capacity suite: this test's value comes from its scale, so FAST skips
/// `::scale::` and FULL runs it (tests/README.md, "Capacity tests").
mod scale {
    use super::*;
    use crate::durable::CanonicalStore;
    use crate::durable::sqlite::request::snapshot::take_request_input_reconstructions;

    /// Exercise real preparation, native start, publication and provider dispatch after
    /// a warmed historical request. Only the explicit final oracle discloses history.
    #[tokio::test]
    async fn full_agent_plain_text_append_reuses_native_input_at_100_300_1000() {
        for count in [100usize, 300, 1000] {
            let adapter = Arc::new(ScriptedAdapter::new(vec![
                workflow_output_turn(&[], Some("warm original response")),
                workflow_output_turn(&[], Some("new original response")),
            ]));
            let store = Arc::new(
                crate::durable::SqliteConversationStore::in_memory(ConversationId::new(
                    "conv_36524fd8-f674-7fc2-8125-06d01fee0e18",
                ))
                .unwrap(),
            );
            let tool_runtime = tool_runtime_with_store(
                "conv_36524fd8-f674-7fc2-8125-06d01fee0e18",
                Some(store.clone()),
            );
            let canonical: Vec<_> = (0..count)
                .map(|index| {
                    MessageBlock::User(inbound_message(
                        &format!("full-chain-{index}"),
                        &format!("original 水🙂 {index}"),
                        UserSource::Human,
                    ))
                })
                .collect();
            let mut first = request(&adapter);
            first.conversation = ConversationState::from_messages(canonical.clone()).unwrap();
            let lease = capability_lease(ToolRegistry::new(), &tool_runtime)
                .await
                .into_lease();
            let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
            let warm = AgentExecution::new(
                first,
                lease,
                &cancellation,
                crate::scripted_suites::support::default_execution_policy(),
                runtime(&adapter),
                &tool_runtime,
                crate::agent::AttemptLifecycle::inert(),
            )
            .unwrap()
            .run()
            .await;
            assert!(matches!(warm.outcome, AttemptOutcome::Completed { .. }));
            let mut second = request(&adapter);
            second.attempt_id = AttemptId::new("full-chain-second");
            second.conversation = warm.conversation;
            let delta = MessageBlock::User(inbound_message(
                "full-chain-new-user",
                "new exact input",
                UserSource::Human,
            ));
            store.append_canonical(&delta).unwrap();
            second.conversation.commit(delta.clone()).unwrap();
            let lease = capability_lease(ToolRegistry::new(), &tool_runtime)
                .await
                .into_lease();
            crate::model::selected_input::take_preparation_visits();
            crate::model::selected_input::take_materialized_items();
            crate::model::selected_input::take_hash_work();
            take_request_input_reconstructions();
            let execution = AgentExecution::new(
                second,
                lease,
                &cancellation,
                crate::scripted_suites::support::default_execution_policy(),
                runtime(&adapter),
                &tool_runtime,
                crate::agent::AttemptLifecycle::inert(),
            )
            .unwrap();
            let result = execution.run().await;
            let visits = crate::model::selected_input::take_preparation_visits();
            let materialized = crate::model::selected_input::take_materialized_items();
            let hashes = crate::model::selected_input::take_hash_work();
            let reconstruction = take_request_input_reconstructions();
            assert!(
                matches!(result.outcome, AttemptOutcome::Completed { .. }),
                "{count}: {:?}",
                result.outcome
            );
            assert_eq!(adapter.request_count(), 2);
            assert!(
                visits < 64,
                "native preparation visits only actualdelta, not oldhistory at {count}: {visits}"
            );
            assert_eq!(
                materialized, 0,
                "shared native input is never flattened during preparation at {count}"
            );
            assert_eq!(
                reconstruction,
                (0, 0),
                "commit evidence replaces a second historical Store reconstruction"
            );
            assert!(
                hashes.0 < 16_384 && hashes.1 < 16_384,
                "only genuine new input is hashed at {count}: {hashes:?}"
            );
            assert_full_agent_input_oracle(store.as_ref(), &adapter, &canonical, &delta, count);
        }
    }
}

fn assert_full_agent_input_oracle(
    store: &crate::durable::SqliteConversationStore,
    adapter: &Arc<ScriptedAdapter>,
    canonical: &[MessageBlock],
    delta: &MessageBlock,
    count: usize,
) {
    let requests = adapter.requests();
    let actual = requests.last().unwrap();
    assert_eq!(
        actual.messages.range(0..count).to_vec(),
        crate::model::input::canonical_input(canonical)
    );
    assert!(
        actual
            .messages
            .iter()
            .any(|item| item.as_canonical() == Some(delta))
    );
    let snapshots = store.read_request_snapshots(None, 16).unwrap().snapshots;
    let snapshot = snapshots
        .iter()
        .find(|snapshot| snapshot.identity.attempt_id == AttemptId::new("full-chain-second"))
        .unwrap();
    assert_eq!(
        store
            .reconstruct_model_request(&snapshot.request_id)
            .unwrap(),
        *actual
    );
    let mut after = None;
    let mut terminals = 0;
    loop {
        let page = store.read_events(after, 32).unwrap();
        if page.events.is_empty() {
            break;
        }
        terminals += page
            .events
            .iter()
            .filter(|event| {
                event.attempt_id.as_ref() == Some(&AttemptId::new("full-chain-second"))
                    && AttemptOutcome::from_terminal_event(&event.event).is_some()
            })
            .count();
        assert!(page.next_sequence > after);
        after = page.next_sequence;
    }
    assert_eq!(terminals, 1);
}
