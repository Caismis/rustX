use crate::agent::cancellation::AgentCancellation;
use crate::agent::execution::AgentExecution;
use crate::agent::execution::test_fixtures::*;
use crate::agent::execution::tools::ForegroundProgressBuffer;
use crate::events::types::{AttemptOutcome, RuntimeEvent};
use crate::message::types::ContentBlockIndex;
use crate::model::event::ModelEvent;
use crate::model::finish::ModelFinishReason;
use crate::runtime::identity::{ConversationId, ToolCallId, ToolId};
use crate::runtime::types::CancellationReason;
use crate::scripted_suites::common::{capability_lease, tool_runtime_with_store};
use crate::tools::executor::{ProgressReporter, ToolExecutor, ToolRegistry};
use crate::tools::limits::{MAX_PROGRESS_EVENTS_PER_FOREGROUND_CALL, MAX_PROGRESS_MESSAGE_BYTES};
use crate::tools::types::{
    ToolCall, ToolDefinition, ToolExecutionPolicy, ToolExecutionResult, ToolExecutionStatus,
    ToolInvocation, ToolOrigin, ToolReplayPolicy,
};
use std::sync::Arc;
use tokio::sync::watch;

/// An empty bounded foreground progress buffer for one scripted call.
fn foreground_buffer() -> ForegroundProgressBuffer {
    ForegroundProgressBuffer::new(ToolCallId::new("call-1"), ToolId::new("tool-1"))
}

/// Reports one numbered progress observation through the reporter seam,
/// exactly as an executor would.
fn report_progress(buffer: &ForegroundProgressBuffer, index: usize) {
    buffer.report(crate::tools::types::ToolProgress {
        message: Some(format!("progress {index}")),
        completed: None,
        total: None,
    });
}

/// Drains the retained progress messages in observation order.
fn retained_messages(buffer: &ForegroundProgressBuffer) -> Vec<String> {
    buffer
        .take()
        .iter()
        .map(|event| match event {
            RuntimeEvent::ToolExecutionProgress { progress, .. } => {
                progress.message.clone().expect("numbered progress message")
            }
            other => {
                panic!("the foreground progress buffer retains only progress events: {other:?}")
            }
        })
        .collect()
}

/// Exact bound: reporting exactly `MAX_PROGRESS_EVENTS_PER_FOREGROUND_CALL`
/// observations retains every one of them in observation order, and the
/// retained count never exceeds the bound while reporting.
#[test]
fn foreground_progress_buffer_retains_the_exact_bound() {
    let buffer = foreground_buffer();
    for index in 0..MAX_PROGRESS_EVENTS_PER_FOREGROUND_CALL {
        report_progress(&buffer, index);
        assert!(
            buffer.retained_len() <= MAX_PROGRESS_EVENTS_PER_FOREGROUND_CALL,
            "the retained count never exceeds the bound"
        );
    }
    let messages = retained_messages(&buffer);
    assert_eq!(messages.len(), MAX_PROGRESS_EVENTS_PER_FOREGROUND_CALL);
    for (index, message) in messages.iter().enumerate() {
        assert_eq!(message, &format!("progress {index}"));
    }
}

/// One over the bound: the first `MAX - 1` observations are pinned and
/// the final slot tracks the newest observation, so the retained count
/// stays exactly at the bound and ends with the latest progress.
#[test]
fn foreground_progress_buffer_one_over_the_bound_keeps_first_prefix_plus_latest() {
    let buffer = foreground_buffer();
    for index in 0..=MAX_PROGRESS_EVENTS_PER_FOREGROUND_CALL {
        report_progress(&buffer, index);
        assert!(
            buffer.retained_len() <= MAX_PROGRESS_EVENTS_PER_FOREGROUND_CALL,
            "the retained count never exceeds the bound"
        );
    }
    let messages = retained_messages(&buffer);
    let mut expected: Vec<String> = (0..MAX_PROGRESS_EVENTS_PER_FOREGROUND_CALL - 1)
        .map(|index| format!("progress {index}"))
        .collect();
    expected.push(format!(
        "progress {MAX_PROGRESS_EVENTS_PER_FOREGROUND_CALL}"
    ));
    assert_eq!(
        messages, expected,
        "the overflow policy is deterministic: first MAX-1 pinned, newest last"
    );
}

/// Flood: a misbehaving executor reporting ten times the bound never
/// grows the buffer past the bound; the retained prefix stays the
/// earliest observations and the final slot is the newest one.
#[test]
fn foreground_progress_buffer_flood_never_exceeds_the_bound() {
    const FLOOD: usize = MAX_PROGRESS_EVENTS_PER_FOREGROUND_CALL * 10;
    let buffer = foreground_buffer();
    for index in 0..FLOOD {
        report_progress(&buffer, index);
        assert!(
            buffer.retained_len() <= MAX_PROGRESS_EVENTS_PER_FOREGROUND_CALL,
            "the bound holds during reporting, not only after settlement"
        );
    }
    let messages = retained_messages(&buffer);
    assert_eq!(messages.len(), MAX_PROGRESS_EVENTS_PER_FOREGROUND_CALL);
    for (index, message) in messages[..MAX_PROGRESS_EVENTS_PER_FOREGROUND_CALL - 1]
        .iter()
        .enumerate()
    {
        assert_eq!(message, &format!("progress {index}"));
    }
    assert_eq!(
        messages.last().expect("retained progress"),
        &format!("progress {}", FLOOD - 1),
        "the final retained observation is the newest executor state"
    );
}

/// The canonical shared normalization (message bytes, finite values)
/// applies as part of bounded retention; the cardinality bound never
/// bypasses or duplicates it.
#[test]
fn foreground_progress_buffer_applies_canonical_normalization_before_retention() {
    let buffer = foreground_buffer();
    buffer.report(crate::tools::types::ToolProgress {
        message: Some("x".repeat(MAX_PROGRESS_MESSAGE_BYTES + 10)),
        completed: Some(f64::NAN),
        total: Some(f64::INFINITY),
    });
    let events = buffer.take();
    assert_eq!(events.len(), 1);
    let RuntimeEvent::ToolExecutionProgress { progress, .. } = &events[0] else {
        panic!("the buffer retains a progress event");
    };
    assert_eq!(
        progress.message.as_deref().expect("message").len(),
        MAX_PROGRESS_MESSAGE_BYTES
    );
    assert_eq!(progress.completed, None, "non-finite values are dropped");
    assert_eq!(progress.total, None, "non-finite values are dropped");
}

/// A foreground executor that reports two live progress observations,
/// parks until the test releases it, and only then settles — so the
/// test observes the exact "reports delivered, tool still executing"
/// cut without any timing assumptions (Issue #178).
struct ParkedProgressTool {
    /// Set once both reports were issued and the executor is parked.
    reported: watch::Sender<bool>,
    /// The test's release; the executor settles after it fires.
    release: Arc<tokio::sync::Notify>,
}

impl ParkedProgressTool {
    fn definition() -> ToolDefinition {
        ToolDefinition {
            id: ToolId::new("tool-progress"),
            name: "progress".to_owned(),
            description: String::new(),
            input_schema: serde_json::json!({"type": "object"}),
            execution_policy: ToolExecutionPolicy::ForegroundOnly,
            replay_policy: ToolReplayPolicy::Never,
            origin: ToolOrigin::Builtin,
        }
    }
}

impl ToolExecutor for ParkedProgressTool {
    fn start<'a>(
        &'a self,
        _invocation: ToolInvocation,
        context: crate::tools::executor::ToolExecutionContext<'a>,
    ) -> crate::tools::executor::ToolExecutionHandle<'a> {
        let cancellation = context.cancellation.clone();
        crate::tools::executor::ToolExecutionHandle::settled_by_operation(
            Box::pin(async move {
                context.progress.report(crate::tools::types::ToolProgress {
                    message: Some("first".to_owned()),
                    completed: Some(1.0),
                    total: Some(2.0),
                });
                context.progress.report(crate::tools::types::ToolProgress {
                    message: Some("second".to_owned()),
                    completed: Some(2.0),
                    total: Some(2.0),
                });
                self.reported.send_replace(true);
                self.release.notified().await;
                ToolExecutionResult {
                    deliveries: Vec::new(),
                    status: ToolExecutionStatus::Success,
                    content: Vec::new(),
                    duration_ms: 1,
                    exit_code: None,
                    artifacts: Vec::new(),
                    truncation: None,
                    workflow: None,
                    managed_output: None,
                }
            }),
            cancellation,
        )
    }

    fn progress_capability(&self) -> crate::tools::deadline::ToolProgressCapability {
        crate::tools::deadline::ToolProgressCapability::None
    }
}

/// Issue #178: with an observation seam installed, a progress-reporting
/// foreground tool's live reports reach `observe_tool_progress` WHILE
/// the tool still executes — before any durable `ToolExecutionProgress`
/// fact commits — and the durable settlement path is unchanged: the
/// retained reports commit at batch settlement, before the completion
/// event.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[allow(clippy::too_many_lines)] // one coherent parked-cut scenario
async fn live_tool_progress_reaches_the_observer_before_settlement() {
    let call = ToolCall {
        id: ToolCallId::new("call-1"),
        tool_id: ToolId::new("tool-progress"),
        name: "progress".to_owned(),
        arguments: crate::scripted_suites::common::tool_call_envelope(serde_json::json!({})),
    };
    let adapter = Arc::new(ScriptedAdapter::new(vec![
        tool_call_script(&call),
        vec![
            ModelEvent::Started,
            ModelEvent::TextDelta {
                block_index: ContentBlockIndex::new(0),
                text: "done".to_owned(),
            },
            ModelEvent::Completed {
                finish_reason: ModelFinishReason::Stop,
                usage: None,
            },
        ],
    ]));
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
    let (reported, mut reported_rx) = watch::channel(false);
    let release = Arc::new(tokio::sync::Notify::new());
    let mut tools = ToolRegistry::new();
    tools
        .register(
            ParkedProgressTool::definition(),
            Arc::new(ParkedProgressTool {
                reported,
                release: Arc::clone(&release),
            }),
        )
        .expect("register progress tool");
    let lease = capability_lease(tools, &tool_runtime).await.into_lease();
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

    // The driver observes the parked mid-execution cut: both live
    // reports have reached the observer, no durable progress fact has
    // committed yet, and only then does the tool settle. Everything is
    // channel/Notify-driven — no sleeps.
    let driver = async {
        reported_rx
            .wait_for(|reported| *reported)
            .await
            .expect("the tool reported and parked");
        let live = observer.tool_progress();
        assert_eq!(
            live.iter()
                .map(|(call_id, progress)| (call_id.as_str(), progress.message.as_deref()))
                .collect::<Vec<_>>(),
            vec![("call-1", Some("first")), ("call-1", Some("second"))],
            "both live reports reached the observer while the tool still executes"
        );
        assert!(
            !observer
                .events()
                .iter()
                .any(|event| matches!(event, RuntimeEvent::ToolExecutionProgress { .. })),
            "live progress is never a committed-fact observation"
        );
        assert!(
            !event_history(store.as_ref())
                .iter()
                .any(|event| matches!(event, RuntimeEvent::ToolExecutionProgress { .. })),
            "nothing durable committed while the tool is parked"
        );
        release.notify_one();
    };
    let (result, ()) = tokio::join!(execution.run(), driver);

    assert!(matches!(
        result.outcome,
        AttemptOutcome::Completed {
            finish_reason: ModelFinishReason::Stop
        }
    ));
    // The durable path is unchanged: exactly the two retained reports
    // committed as canonical facts, in observation order, before the
    // completion event of their call.
    let events = event_history(store.as_ref());
    let progress_positions: Vec<usize> = events
        .iter()
        .enumerate()
        .filter_map(|(index, event)| match event {
            RuntimeEvent::ToolExecutionProgress { tool_call_id, .. }
                if *tool_call_id == ToolCallId::new("call-1") =>
            {
                Some(index)
            }
            _ => None,
        })
        .collect();
    let completed_position = events
        .iter()
        .position(|event| matches!(event, RuntimeEvent::ToolExecutionCompleted { .. }))
        .expect("the completion fact committed");
    assert_eq!(
        progress_positions.len(),
        2,
        "the retained reports commit once each"
    );
    assert!(
        progress_positions
            .iter()
            .all(|position| *position < completed_position),
        "progress facts precede their completion event"
    );
    let messages: Vec<&str> = progress_positions
        .iter()
        .map(|position| match &events[*position] {
            RuntimeEvent::ToolExecutionProgress { progress, .. } => {
                progress.message.as_deref().expect("message")
            }
            _ => unreachable!("positions were filtered to progress events"),
        })
        .collect();
    assert_eq!(messages, vec!["first", "second"]);
}

/// P1: after Plan authorizes workspace-root `PLAN.md`, replacing that path with
/// an external symlink before execute must refuse the old authorization. The
/// park hook sits after `ToolExecutionStarted` and before the executor runs.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[allow(clippy::too_many_lines)] // one authorize→swap→execute race path
async fn plan_write_refuses_when_authorized_plan_target_is_replaced_before_execute() {
    use crate::agent::execution::test_sync::ToolStartPause;
    use crate::agent::lifecycle::ConfiguredApprovalPolicy;
    use crate::message::types::MessageBlock;
    use crate::runtime::ApprovalMode;
    use crate::tools::types::ToolInvocationPolicy;

    let call = ToolCall {
        id: ToolCallId::new("call-plan"),
        tool_id: ToolId::new("tool-write"),
        name: "write".to_owned(),
        arguments: crate::scripted_suites::common::tool_call_envelope(serde_json::json!({
            "path": "PLAN.md",
            "content": "authorized plan body\n"
        })),
    };
    let adapter = Arc::new(ScriptedAdapter::new(vec![
        tool_call_script(&call),
        vec![
            ModelEvent::Started,
            ModelEvent::TextDelta {
                block_index: ContentBlockIndex::new(0),
                text: "done".to_owned(),
            },
            ModelEvent::Completed {
                finish_reason: ModelFinishReason::Stop,
                usage: None,
            },
        ],
    ]));
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
    let workspace_root = tool_runtime.workspace().root().to_path_buf();
    let plan_path = workspace_root.join("PLAN.md");
    std::fs::write(&plan_path, "seed\n").expect("seed PLAN.md");
    let outside = tempfile::tempdir().expect("outside");
    let external_target = outside.path().join("secret.txt");

    let mut tools = ToolRegistry::new();
    crate::tools::register_native_tools(
        &mut tools,
        crate::tools::NativeToolResources {
            background: tool_runtime.background().clone(),
            subagents: None,
            subagent_catalog: crate::runtime::subagent::AgentCatalog::empty(),
        },
        crate::tools::NativeToolPolicies {
            write: ToolInvocationPolicy::new(
                crate::tools::types::ToolExecutionPolicy::ForegroundOnly,
            ),
            ..crate::tools::NativeToolPolicies::default()
        },
    )
    .expect("register native tools");
    let lease = capability_lease(tools, &tool_runtime).await.into_lease();
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let (pause, mut reached, release) = ToolStartPause::install();
    let mut execution = AgentExecution::new(
        request(&adapter),
        lease,
        &cancellation,
        crate::scripted_suites::support::default_execution_policy(),
        runtime(&adapter),
        &tool_runtime,
        crate::agent::AttemptLifecycle::inert()
            .with_pre_tool_policy(Arc::new(ConfiguredApprovalPolicy::new(ApprovalMode::Plan))),
    )
    .expect("execution construction");
    execution.install_tool_start_pause(pause);

    // ToolStartPause parks with a blocking recv after start. Wait/swap on a
    // dedicated thread so the parked worker can be released.
    let swap_plan = plan_path.clone();
    let swap_external = external_target.clone();
    let swap = std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("swap runtime");
        runtime.block_on(async {
            reached
                .wait_for(|parked| *parked)
                .await
                .expect("tool-start park stays open");
            std::fs::remove_file(&swap_plan).expect("remove authorized PLAN.md");
            std::os::unix::fs::symlink(&swap_external, &swap_plan)
                .expect("replace PLAN.md with external symlink");
            release.send(()).expect("release execute frontier");
        });
    });
    let result = execution.run().await;
    swap.join().expect("swap thread");

    assert!(matches!(
        result.outcome,
        AttemptOutcome::Completed {
            finish_reason: ModelFinishReason::Stop
        }
    ));
    let messages = result.messages();
    let status = messages
        .iter()
        .find_map(|message| match message {
            MessageBlock::Tool(tool) if tool.tool_call_id.as_str() == "call-plan" => {
                Some(&tool.result.status)
            }
            _ => None,
        })
        .expect("plan write result");
    match status {
        ToolExecutionStatus::Failed { error } => {
            assert!(
                error.contains("mutation target changed since authorization"),
                "expected bind-contract failure, got {error}"
            );
        }
        other => panic!("expected Failed status, got {other:?}"),
    }
    assert!(
        !external_target.exists(),
        "external symlink destination must remain untouched"
    );
    assert!(
        std::fs::symlink_metadata(&plan_path)
            .expect("plan path metadata")
            .file_type()
            .is_symlink(),
        "PLAN.md remains the swapped symlink rather than being rewritten as the authorized file"
    );
}
