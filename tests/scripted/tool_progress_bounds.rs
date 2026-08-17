//! Deterministic regressions for the foreground tool-progress cardinality
//! bound.
//!
//! One active foreground tool invocation retains only a finite number of
//! progress observations before structural settlement: each observation is
//! payload-bounded by the canonical shared normalization
//! (`bound_tool_progress` / `MAX_PROGRESS_MESSAGE_BYTES`) and the retained
//! count is bounded by `MAX_PROGRESS_EVENTS_PER_FOREGROUND_CALL`. Once the
//! bound is reached, the first `MAX - 1` observations are pinned and the
//! final slot tracks the newest observation, so the retained state always
//! ends with the most recent executor progress. Only retained observations
//! become durable `ToolExecutionProgress` Event Journal facts at batch
//! commit; coalesced observations never cross the durable commit point.
//!
//! All synchronization is explicit (watch channels and controller tasks);
//! no wall-clock sleep proves any concurrency invariant.

#![allow(clippy::similar_names)] // scripted fixture names are intentionally similar

use super::{common, support};

use std::sync::Arc;

use rustx::agent::{AgentCancellation, AgentExecution, AgentExecutionRequest};
use rustx::events::types::{AttemptOutcome, RuntimeEvent};
use rustx::message::types::{
    MessageBlock, ToolMessageBlock, UserContentBlock, UserMessageBlock, UserSource,
};
use rustx::model::event::ModelEvent;
use rustx::model::finish::ModelFinishReason;
use rustx::runtime::identity::{AgentId, AttemptId, ConversationId, MessageId};
use rustx::runtime::types::CancellationReason;
use rustx::tools::executor::ToolRegistry;
use rustx::tools::limits::MAX_PROGRESS_EVENTS_PER_FOREGROUND_CALL;
use rustx::tools::types::{ToolConcurrencyPolicy, ToolExecutionPolicy, ToolExecutionStatus};
use support::fake::{
    FakeModel, FakeStep, FakeTool, ScriptedCall, await_started, fake_model, success_result,
    tool_call_events,
};

fn request(model: &std::sync::Arc<FakeModel>) -> AgentExecutionRequest {
    AgentExecutionRequest {
        agent_id: AgentId::new("agent-a"),
        conversation_id: ConversationId::new("conv-1"),
        attempt_id: AttemptId::new("attempt-1"),
        conversation: rustx::conversation::ConversationState::from_messages(vec![
            MessageBlock::User(UserMessageBlock {
                id: MessageId::new("msg-user-1"),
                content: vec![UserContentBlock::Text(rustx::message::content::TextBlock {
                    text: "go".to_owned(),
                })],
                source: UserSource::Human,
                kind: rustx::message::types::InboundKind::Message,
                timestamp: None,
            }),
        ])
        .expect("bootstrap conversation"),
        initial_turn_trigger: rustx::agent::InitialTurnTrigger::Continuation,
        timezone: None,
        model: support::attempt_model(model.clone(), "fake-model"),
    }
}

fn runtime(model: &std::sync::Arc<FakeModel>) -> rustx::context::ContextRuntime {
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
        rustx::context::AgentStatusComposer::default(),
        &snapshot,
    )
    .expect("valid context runtime")
}

async fn run(
    model: &std::sync::Arc<FakeModel>,
    tools: ToolRegistry,
    cancellation: &AgentCancellation,
) -> common::DurableExecutionAudit {
    let tool_runtime = common::tool_runtime("conv-1");
    let store = tool_runtime.durable_store();
    let capability = common::capability_lease(tools, &tool_runtime).await;
    let result = AgentExecution::new(
        request(model),
        capability.into_lease(),
        cancellation,
        runtime(model),
        &tool_runtime,
        rustx::agent::AttemptLifecycle::inert(),
    )
    .expect("conversation identity matches the tool runtime")
    .run()
    .await;
    common::durable_agent_result(result, store.as_ref())
}

/// A tool-call turn scripted from calls; the following turn is a Stop.
fn tool_turn_then_stop(calls: &[&ScriptedCall]) -> Vec<Vec<FakeStep>> {
    let mut first = vec![FakeStep::Emit(ModelEvent::Started)];
    for (block, call) in calls.iter().enumerate() {
        let block = u32::try_from(block).expect("scripted blocks fit in u32");
        for event in tool_call_events(block, call) {
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

fn scripted(id: &str, tool_id: &str, name: &str, arguments: serde_json::Value) -> ScriptedCall {
    ScriptedCall {
        id: Box::leak(id.to_owned().into_boxed_str()),
        tool_id: Box::leak(tool_id.to_owned().into_boxed_str()),
        name: Box::leak(name.to_owned().into_boxed_str()),
        arguments,
    }
}

/// A foreground-only fake tool emitting `progress` numbered progress
/// observations per call, with the given concurrency policy.
fn progress_tool(name: &str, tool_id: &str, concurrency: ToolConcurrencyPolicy) -> FakeTool {
    FakeTool::new(
        common::tool_policies(
            name,
            tool_id,
            ToolExecutionPolicy::ForegroundOnly,
            concurrency,
        ),
        success_result("ok"),
    )
}

/// The durable `ToolExecutionProgress` messages of one call, in journal
/// order.
fn durable_progress_messages(result: &common::DurableExecutionAudit, call_id: &str) -> Vec<String> {
    result
        .event_history
        .iter()
        .filter_map(|event| match event {
            RuntimeEvent::ToolExecutionProgress {
                tool_call_id,
                progress,
                ..
            } if tool_call_id.as_str() == call_id => {
                Some(progress.message.clone().expect("numbered progress message"))
            }
            _ => None,
        })
        .collect()
}

/// The durable tool lifecycle event sequence of one attempt, reduced to
/// progress markers carrying their call identity.
fn lifecycle_sequence(result: &common::DurableExecutionAudit) -> Vec<String> {
    result
        .event_history
        .iter()
        .filter_map(|event| match event {
            RuntimeEvent::ToolExecutionStarted { tool_call_id, .. } => {
                Some(format!("started:{}", tool_call_id.as_str()))
            }
            RuntimeEvent::ToolExecutionProgress { tool_call_id, .. } => {
                Some(format!("progress:{}", tool_call_id.as_str()))
            }
            RuntimeEvent::ToolExecutionCompleted { tool_call_id, .. } => {
                Some(format!("completed:{}", tool_call_id.as_str()))
            }
            _ => None,
        })
        .collect()
}

/// Tool messages committed to canonical history in order.
fn tool_messages(result: &common::DurableExecutionAudit) -> Vec<&ToolMessageBlock> {
    result
        .messages()
        .iter()
        .filter_map(|message| match message {
            MessageBlock::Tool(tool) => Some(tool),
            _ => None,
        })
        .collect()
}

/// Exact bound: a call reporting exactly `MAX_PROGRESS_EVENTS_PER_FOREGROUND_CALL`
/// observations commits every one of them as durable progress facts, in
/// observation order, before its completion event. The canonical tool result
/// is unchanged.
#[tokio::test]
async fn foreground_progress_at_the_exact_bound_is_committed_verbatim() {
    let call = scripted("call-1", "tool-alpha", "alpha", serde_json::json!({}));
    let model = fake_model(tool_turn_then_stop(&[&call]));
    let mut tools = ToolRegistry::new();
    progress_tool("alpha", "tool-alpha", ToolConcurrencyPolicy::Sequential)
        .emitting_progress(MAX_PROGRESS_EVENTS_PER_FOREGROUND_CALL)
        .register(&mut tools);
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);

    let result = run(&model, tools, &cancellation).await;

    assert!(matches!(
        result.outcome,
        AttemptOutcome::Completed {
            finish_reason: ModelFinishReason::Stop
        }
    ));
    let messages = tool_messages(&result);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].result.status, ToolExecutionStatus::Success);

    let progress = durable_progress_messages(&result, "call-1");
    assert_eq!(
        progress.len(),
        MAX_PROGRESS_EVENTS_PER_FOREGROUND_CALL,
        "every observation up to the bound becomes a durable progress fact"
    );
    for (index, message) in progress.iter().enumerate() {
        assert_eq!(message, &format!("progress {index}"));
    }
    let lifecycle = lifecycle_sequence(&result);
    let started = lifecycle
        .iter()
        .position(|event| event == "started:call-1")
        .expect("started event");
    let completed = lifecycle
        .iter()
        .position(|event| event == "completed:call-1")
        .expect("completed event");
    assert!(
        lifecycle[started + 1..completed]
            .iter()
            .all(|event| event == "progress:call-1"),
        "progress facts precede the completion event of their call"
    );
}

/// One over the bound: the durable journal retains exactly the bound —
/// the first `MAX - 1` observations pinned, the final slot tracking the
/// newest observation. The coalesced middle observation never becomes a
/// durable fact.
#[tokio::test]
async fn foreground_progress_one_over_the_bound_keeps_first_prefix_plus_latest() {
    let call = scripted("call-1", "tool-alpha", "alpha", serde_json::json!({}));
    let model = fake_model(tool_turn_then_stop(&[&call]));
    let mut tools = ToolRegistry::new();
    progress_tool("alpha", "tool-alpha", ToolConcurrencyPolicy::Sequential)
        .emitting_progress(MAX_PROGRESS_EVENTS_PER_FOREGROUND_CALL + 1)
        .register(&mut tools);
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);

    let result = run(&model, tools, &cancellation).await;

    assert!(matches!(
        result.outcome,
        AttemptOutcome::Completed {
            finish_reason: ModelFinishReason::Stop
        }
    ));
    let progress = durable_progress_messages(&result, "call-1");
    let mut expected: Vec<String> = (0..MAX_PROGRESS_EVENTS_PER_FOREGROUND_CALL - 1)
        .map(|index| format!("progress {index}"))
        .collect();
    expected.push(format!(
        "progress {MAX_PROGRESS_EVENTS_PER_FOREGROUND_CALL}"
    ));
    assert_eq!(
        progress, expected,
        "the deterministic overflow policy retains the first MAX-1 \
         observations plus the newest one"
    );
}

/// Flood: a misbehaving executor reporting ten times the bound still
/// commits exactly the bounded retained prefix-plus-latest progress facts,
/// the batch settles normally, and the lifecycle order is unchanged.
#[tokio::test]
async fn foreground_progress_flood_is_bounded_and_still_settles() {
    const FLOOD: usize = MAX_PROGRESS_EVENTS_PER_FOREGROUND_CALL * 10;
    let call = scripted("call-1", "tool-alpha", "alpha", serde_json::json!({}));
    let model = fake_model(tool_turn_then_stop(&[&call]));
    let mut tools = ToolRegistry::new();
    progress_tool("alpha", "tool-alpha", ToolConcurrencyPolicy::Sequential)
        .emitting_progress(FLOOD)
        .register(&mut tools);
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);

    let result = run(&model, tools, &cancellation).await;

    assert!(matches!(
        result.outcome,
        AttemptOutcome::Completed {
            finish_reason: ModelFinishReason::Stop
        }
    ));
    let messages = tool_messages(&result);
    assert_eq!(messages.len(), 1, "the canonical batch still settles");
    assert_eq!(messages[0].result.status, ToolExecutionStatus::Success);

    let progress = durable_progress_messages(&result, "call-1");
    assert_eq!(
        progress.len(),
        MAX_PROGRESS_EVENTS_PER_FOREGROUND_CALL,
        "the durable journal contains only the bounded retained progress facts"
    );
    for (index, message) in progress[..MAX_PROGRESS_EVENTS_PER_FOREGROUND_CALL - 1]
        .iter()
        .enumerate()
    {
        assert_eq!(message, &format!("progress {index}"));
    }
    assert_eq!(
        progress.last().expect("retained progress"),
        &format!("progress {}", FLOOD - 1),
        "the final durable progress fact is the newest executor state"
    );
    let lifecycle = lifecycle_sequence(&result);
    assert_eq!(
        lifecycle.first().map(String::as_str),
        Some("started:call-1")
    );
    assert_eq!(
        lifecycle.last().map(String::as_str),
        Some("completed:call-1"),
        "the normal lifecycle brackets the bounded progress facts"
    );
}

/// Parallel calls: the bound is per call. Two parallel calls each flood
/// progress and complete in reversed physical order; the durable journal
/// still groups each call's bounded progress before its completion event in
/// canonical model-call order.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn parallel_calls_bound_progress_per_call_and_keep_canonical_order() {
    const FLOOD: usize = MAX_PROGRESS_EVENTS_PER_FOREGROUND_CALL * 2;
    let call_a = scripted("call-a", "tool-alpha", "alpha", serde_json::json!({"n": 1}));
    let call_b = scripted("call-b", "tool-beta", "beta", serde_json::json!({"n": 2}));
    let model = fake_model(tool_turn_then_stop(&[&call_a, &call_b]));
    let (tool_a, release_a) = FakeTool::parking(
        common::tool_policies(
            "alpha",
            "tool-alpha",
            ToolExecutionPolicy::ForegroundOnly,
            ToolConcurrencyPolicy::Parallel,
        ),
        success_result("a"),
    );
    let (tool_b, release_b) = FakeTool::parking(
        common::tool_policies(
            "beta",
            "tool-beta",
            ToolExecutionPolicy::ForegroundOnly,
            ToolConcurrencyPolicy::Parallel,
        ),
        success_result("b"),
    );
    let tool_a = tool_a.emitting_progress(FLOOD);
    let tool_b = tool_b.emitting_progress(FLOOD);
    let mut started_a = tool_a.started();
    let mut started_b = tool_b.started();
    let completed_b = tool_b.completed();
    let mut controller_completed_b = completed_b.clone();
    let mut tools = ToolRegistry::new();
    tool_a.register(&mut tools);
    tool_b.register(&mut tools);
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);

    // Both calls start concurrently and flood progress; B completes
    // physically first, then A.
    let controller = tokio::spawn(async move {
        await_started(&mut started_a, "A started").await;
        await_started(&mut started_b, "B started").await;
        release_b.send_replace(true);
        controller_completed_b
            .wait_for(|order| order.iter().any(|name| name == "beta"))
            .await
            .expect("B completed physically first");
        release_a.send_replace(true);
    });
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        run(&model, tools, &cancellation),
    )
    .await
    .expect("run terminates");
    controller.await.expect("controller task");

    assert!(matches!(
        result.outcome,
        AttemptOutcome::Completed {
            finish_reason: ModelFinishReason::Stop
        }
    ));
    let messages = tool_messages(&result);
    assert_eq!(
        messages
            .iter()
            .map(|message| message.tool_call_id.as_str())
            .collect::<Vec<_>>(),
        vec!["call-a", "call-b"],
        "canonical ToolMessage order is the original model call order"
    );

    for call_id in ["call-a", "call-b"] {
        let progress = durable_progress_messages(&result, call_id);
        assert_eq!(
            progress.len(),
            MAX_PROGRESS_EVENTS_PER_FOREGROUND_CALL,
            "the cardinality bound is per call: {call_id}"
        );
        assert_eq!(
            progress.last().expect("retained progress"),
            &format!("progress {}", FLOOD - 1),
            "each call's final retained observation is its newest state"
        );
    }

    let lifecycle = lifecycle_sequence(&result);
    let expected: Vec<String> = ["started:call-a", "started:call-b"]
        .into_iter()
        .map(str::to_owned)
        .chain(std::iter::repeat_n(
            "progress:call-a".to_owned(),
            MAX_PROGRESS_EVENTS_PER_FOREGROUND_CALL,
        ))
        .chain(["completed:call-a".to_owned()])
        .chain(std::iter::repeat_n(
            "progress:call-b".to_owned(),
            MAX_PROGRESS_EVENTS_PER_FOREGROUND_CALL,
        ))
        .chain(["completed:call-b".to_owned()])
        .collect();
    assert_eq!(
        lifecycle, expected,
        "physical completion order never influences the canonical durable order"
    );
}
