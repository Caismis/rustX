//! M5 background execution tests.
//!
//! These tests prove the conversation-owned background registry contracts:
//! deterministic `exec_N` allocation, the two-stage dispatch ownership
//! commit, the cancel-vs-completion linearization rule, exactly-once
//! terminal settlement and mailbox publication, bounded progress
//! snapshots, cross-conversation isolation, the `background_task`
//! intrinsic, the mailbox boundary races for terminal inbound
//! notifications, and the runtime-owned Agent Status background section.
//! All concurrency is driven by explicit gates (watches and channels); no
//! wall-clock sleep proves any invariant.

#![allow(clippy::similar_names)] // scripted fixture names are intentionally similar

use super::{common, support};

use std::sync::Arc;
use std::time::Duration;

use rustx::agent::{
    AgentCancellation, AgentExecution, AgentExecutionRequest, AgentExecutionResult,
};
use rustx::events::types::{AttemptOutcome, RuntimeEvent};
use rustx::events::{RecordingEventSink, RuntimeEventSink};
use rustx::message::types::{
    MessageBlock, ToolMessageBlock, UserContentBlock, UserMessageBlock, UserSource,
};
use rustx::model::event::ModelEvent;
use rustx::model::finish::ModelFinishReason;
use rustx::runtime::identity::{
    AgentId, AttemptId, ConversationId, MessageId, ToolCallId, ToolExecutionId, ToolId,
};
use rustx::runtime::inbound::ConversationInboundMailbox;
use rustx::runtime::types::CancellationReason;
use rustx::tools::artifacts::ArtifactStore;
use rustx::tools::background::{
    BackgroundDispatchOutcome, BackgroundExecutionSnapshot, BackgroundLifecycle,
    BackgroundResources, ConversationBackgroundRegistry,
};
use rustx::tools::executor::{ToolExecutionContext, ToolExecutor, ToolRegistry};
use rustx::tools::runtime::ConversationToolRuntime;
use rustx::tools::types::{
    ToolCall, ToolConcurrencyPolicy, ToolExecutionPolicy, ToolExecutionResult, ToolExecutionStatus,
    ToolInvocation, ToolInvocationMode, ToolProgress, ToolResultContent,
};
use rustx::tools::workspace::Workspace;
use support::fake::{
    FakeModel, FakeStep, FakeTool, ScriptedCall, fake_model, success_result, tool_call_events,
};

/// A fixed runtime clock.
#[derive(Debug, Clone, Copy)]
struct FixedRuntimeClock(chrono::DateTime<chrono::Utc>);

impl rustx::runtime::RuntimeClock for FixedRuntimeClock {
    fn now(&self) -> chrono::DateTime<chrono::Utc> {
        self.0
    }
}

fn utc(rfc3339: &str) -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::parse_from_rfc3339(rfc3339)
        .expect("fixed timestamp")
        .with_timezone(&chrono::Utc)
}

/// A controlled deterministic executor: reports scripted progress, parks
/// until released or cancelled, and always settles.
struct ControlledExecutor {
    started: tokio::sync::watch::Sender<bool>,
    release: Option<tokio::sync::watch::Sender<bool>>,
    result: ToolExecutionResult,
    progress: Vec<ToolProgress>,
}

impl ControlledExecutor {
    fn instant(result: ToolExecutionResult) -> (Self, tokio::sync::watch::Receiver<bool>) {
        let (started, started_rx) = tokio::sync::watch::channel(false);
        (
            Self {
                started,
                release: None,
                result,
                progress: Vec::new(),
            },
            started_rx,
        )
    }

    fn parking(
        result: ToolExecutionResult,
    ) -> (
        Self,
        tokio::sync::watch::Receiver<bool>,
        tokio::sync::watch::Sender<bool>,
    ) {
        let (started, started_rx) = tokio::sync::watch::channel(false);
        let (release, _release_rx) = tokio::sync::watch::channel(false);
        (
            Self {
                started,
                release: Some(release.clone()),
                result,
                progress: Vec::new(),
            },
            started_rx,
            release,
        )
    }

    fn with_progress(mut self, progress: Vec<ToolProgress>) -> Self {
        self.progress = progress;
        self
    }
}

impl ToolExecutor for ControlledExecutor {
    fn execute<'a>(
        &'a self,
        invocation: ToolInvocation,
        context: ToolExecutionContext<'a>,
    ) -> futures_util::future::BoxFuture<'a, ToolExecutionResult> {
        let _ = invocation;
        let started = self.started.clone();
        let mut release = self
            .release
            .as_ref()
            .map(tokio::sync::watch::Sender::subscribe);
        let result = self.result.clone();
        let progress = self.progress.clone();
        Box::pin(async move {
            started.send_replace(true);
            for item in progress {
                context.progress.report(item);
            }
            if let Some(release) = release.as_mut() {
                tokio::select! {
                    biased;
                    () = context.cancellation.cancelled() => {
                        return ToolExecutionResult {
                            status: ToolExecutionStatus::Cancelled {
                                reason: CancellationReason::UserRequested,
                            },
                            content: Vec::new(),
                            duration_ms: 0,
                            exit_code: None,
                            artifacts: Vec::new(),
                            truncation: None,
                            managed_output: None,
                        };
                    }
                    released = release.wait_for(|released| *released) => {
                        released.expect("controlled executor release channel stays open");
                    }
                }
            }
            result
        })
    }
}

fn background_invocation(tool: &str) -> ToolInvocation {
    ToolInvocation {
        call_id: ToolCallId::new("call-1"),
        tool_id: ToolId::new(format!("tool-{tool}")),
        tool_name: tool.to_owned(),
        mode: ToolInvocationMode::Background,
        arguments: serde_json::json!({}),
    }
}

/// A background fixture: isolated workspace/artifacts, a shared mailbox, a
/// fixed clock, and an optional recording event sink.
struct BackgroundFixture {
    _dir: tempfile::TempDir,
    registry: ConversationBackgroundRegistry,
    mailbox: ConversationInboundMailbox,
    sink: Option<RecordingEventSink>,
}

fn background_fixture(conversation_id: &str) -> BackgroundFixture {
    background_fixture_with_sink(conversation_id, true)
}

fn background_fixture_with_sink(conversation_id: &str, with_sink: bool) -> BackgroundFixture {
    let dir = tempfile::tempdir().expect("temp dir");
    let workspace_root = dir.path().join("workspace");
    std::fs::create_dir_all(&workspace_root).expect("workspace");
    let artifacts = dir.path().join("artifacts");
    let conversation = ConversationId::new(conversation_id);
    let mailbox = ConversationInboundMailbox::new(conversation.clone());
    let sink = with_sink.then(RecordingEventSink::new);
    let resources = BackgroundResources {
        mailbox: mailbox.clone(),
        workspace: Workspace::new(&workspace_root).expect("workspace"),
        artifacts: ArtifactStore::new(conversation.clone(), &artifacts).expect("artifacts"),
        tool_output: rustx::tools::managed_output::ManagedToolOutput::new(
            conversation.clone(),
            artifacts.join("tool-output"),
        )
        .expect("managed tool output"),
        clock: Arc::new(FixedRuntimeClock(utc("2026-08-09T12:00:00Z"))),
        event_sink: sink
            .clone()
            .map(|sink| Arc::new(sink) as Arc<dyn RuntimeEventSink>),
    };
    BackgroundFixture {
        _dir: dir,
        registry: ConversationBackgroundRegistry::new(conversation, resources),
        mailbox,
        sink,
    }
}

fn success() -> ToolExecutionResult {
    ToolExecutionResult {
        status: ToolExecutionStatus::Success,
        content: Vec::new(),
        duration_ms: 0,
        exit_code: None,
        artifacts: Vec::new(),
        truncation: None,
        managed_output: None,
    }
}

fn cancelled() -> ToolExecutionResult {
    ToolExecutionResult {
        status: ToolExecutionStatus::Cancelled {
            reason: CancellationReason::UserRequested,
        },
        content: Vec::new(),
        duration_ms: 0,
        exit_code: None,
        artifacts: Vec::new(),
        truncation: None,
        managed_output: None,
    }
}

/// Dispatch one execution to completion through the fixture.
async fn dispatch_to_terminal(fixture: &BackgroundFixture) -> ToolExecutionId {
    let (executor, started) = ControlledExecutor::instant(success());
    let mut started = started;
    let registry = fixture.registry.clone();
    let prepared = registry
        .prepare_dispatch(
            &background_invocation("bash"),
            &(Arc::new(executor) as Arc<dyn rustx::tools::executor::ToolExecutor>),
            rustx::tools::environment::ToolEnvironment::new(),
            None,
        )
        .expect("prepare");
    let outcome = registry
        .commit_dispatch(prepared, &rustx::runtime::CancellationSignal::new())
        .expect("dispatch commits");
    let BackgroundDispatchOutcome::Accepted { execution_id, .. } = outcome else {
        panic!("accepted");
    };
    await_background_started(&mut started, "runner started").await;
    execution_id
}

/// Deterministic `exec_1`, `exec_2`, ... allocation under the registry
/// synchronization boundary.
#[tokio::test]
async fn execution_ids_are_deterministic_and_monotonic() {
    let fixture = background_fixture("conv-bg");
    let first = dispatch_to_terminal(&fixture).await;
    assert_eq!(first.as_str(), "exec_1");
    let second = dispatch_to_terminal(&fixture).await;
    assert_eq!(second.as_str(), "exec_2");
    let third = dispatch_to_terminal(&fixture).await;
    assert_eq!(third.as_str(), "exec_3");
}

/// The runner cannot begin before the dispatch commit gate is released.
#[tokio::test]
async fn runner_cannot_begin_before_commit_gate() {
    let fixture = background_fixture("conv-bg");
    let (executor, mut started) = ControlledExecutor::instant(success());
    let registry = fixture.registry.clone();
    let prepared = registry
        .prepare_dispatch(
            &background_invocation("bash"),
            &(Arc::new(executor) as Arc<dyn rustx::tools::executor::ToolExecutor>),
            rustx::tools::environment::ToolEnvironment::new(),
            None,
        )
        .expect("prepare");
    // The runner is parked behind the gate: no execution work has begun.
    assert!(
        !*started.borrow(),
        "the runner must not begin before the commit gate"
    );
    let outcome = registry
        .commit_dispatch(prepared, &rustx::runtime::CancellationSignal::new())
        .expect("dispatch commits");
    assert!(matches!(
        outcome,
        BackgroundDispatchOutcome::Accepted { .. }
    ));
    await_background_started(&mut started, "runner started only after the commit gate").await;
}

/// Dispatch vs attempt cancellation: cancellation before the ownership
/// commit rolls the prepared dispatch back — no accepted result, no
/// detached execution.
#[tokio::test]
async fn cancellation_before_ownership_commit_rolls_back() {
    let fixture = background_fixture("conv-bg");
    let (executor, mut started) = ControlledExecutor::instant(success());
    let registry = fixture.registry.clone();
    let prepared = registry
        .prepare_dispatch(
            &background_invocation("bash"),
            &(Arc::new(executor) as Arc<dyn rustx::tools::executor::ToolExecutor>),
            rustx::tools::environment::ToolEnvironment::new(),
            None,
        )
        .expect("prepare");
    let attempt_cancellation = rustx::runtime::CancellationSignal::new();
    attempt_cancellation.cancel();
    let outcome = registry
        .commit_dispatch(prepared, &attempt_cancellation)
        .expect("dispatch commits");
    assert_eq!(
        outcome,
        BackgroundDispatchOutcome::RolledBack,
        "attempt cancellation before the commit means no accepted result"
    );
    let started_outcome = tokio::time::timeout(
        Duration::from_millis(200),
        started.wait_for(|started| *started),
    )
    .await;
    assert!(
        !matches!(started_outcome, Ok(Ok(_))),
        "the rolled-back runner must never begin"
    );
    assert_eq!(
        fixture.registry.all_snapshots().len(),
        0,
        "no detached execution exists"
    );
}

/// Ownership commit vs attempt cancellation: the commit winner means the
/// background stays conversation-owned and the accepted result commits even
/// when the attempt cancels afterwards.
#[tokio::test]
async fn ownership_commit_wins_over_later_attempt_cancellation() {
    let fixture = background_fixture("conv-bg");
    let (executor, mut started, _release) = ControlledExecutor::parking(success());
    let registry = fixture.registry.clone();
    let prepared = registry
        .prepare_dispatch(
            &background_invocation("bash"),
            &(Arc::new(executor) as Arc<dyn rustx::tools::executor::ToolExecutor>),
            rustx::tools::environment::ToolEnvironment::new(),
            None,
        )
        .expect("prepare");
    let attempt_cancellation = rustx::runtime::CancellationSignal::new();
    let outcome = registry
        .commit_dispatch(prepared, &attempt_cancellation)
        .expect("dispatch commits");
    let BackgroundDispatchOutcome::Accepted {
        execution_id,
        result,
    } = outcome
    else {
        panic!("accepted");
    };
    // Attempt cancellation after the commit cannot reclaim the work.
    attempt_cancellation.cancel();
    await_background_started(&mut started, "conversation-owned runner still starts").await;
    let snapshot = registry.snapshot(&execution_id).expect("snapshot");
    assert_eq!(snapshot.state, BackgroundLifecycle::Running);
    let accepted = match &result.content[0] {
        ToolResultContent::Json { value } => value.clone(),
        other => panic!("expected JSON, got {other:?}"),
    };
    assert_eq!(accepted["execution_id"], "exec_1");
    assert_eq!(accepted["state"], "starting");
    assert_eq!(accepted["tool"], "bash");
    // Issue #86: the accepted result advertises the stable read-only
    // live-output locator, allocated at dispatch time.
    let output_path = accepted["output_path"].as_str().expect("output_path");
    assert!(std::path::Path::new(output_path).is_absolute());
    assert!(
        output_path.ends_with("tasks/exec_1.output"),
        "{output_path}"
    );
    assert!(std::path::Path::new(output_path).exists());
    let note = accepted["note"].as_str().expect("note");
    assert!(
        note.contains("Read or Grep"),
        "the continuation guidance travels with the locator"
    );
    assert!(
        note.contains("when produced"),
        "the wording is precise: only streaming executors append live output: {note}"
    );
    assert!(
        !note.contains("is being written"),
        "no blanket streaming promise for non-streaming executors: {note}"
    );
}

/// Natural completion before cancel: completion wins, and a later cancel is
/// an idempotent no-op returning the terminal snapshot.
#[tokio::test]
async fn natural_completion_wins_over_later_cancel() {
    let fixture = background_fixture("conv-bg");
    let execution_id = dispatch_to_terminal(&fixture).await;
    let terminal = fixture.registry.snapshot(&execution_id).expect("snapshot");
    assert_eq!(terminal.state, BackgroundLifecycle::Succeeded);
    // The terminal transition published exactly one inbound message.
    let batch = fixture
        .mailbox
        .select_pending_batch()
        .expect("select")
        .expect("one batch");
    assert_eq!(batch.items().len(), 1);
    assert_eq!(
        batch.items()[0].message().id.as_str(),
        "background-exec_1-terminal",
        "deterministic terminal message identity"
    );
    let _ = fixture.mailbox.adopt_pending_batch(&batch).expect("adopt");
    // A later cancel is an idempotent no-op returning the terminal snapshot.
    let after_cancel = fixture
        .registry
        .cancel(&execution_id)
        .expect("cancel returns the snapshot");
    assert_eq!(after_cancel, terminal);
    assert!(
        fixture
            .mailbox
            .select_pending_batch()
            .expect("select")
            .is_none(),
        "no second publication"
    );
}

/// Cancel transition before completion: cancellation wins and owns
/// settlement; the later executor return cannot overwrite it with a
/// success.
#[tokio::test]
async fn cancel_before_completion_wins_settlement() {
    let fixture = background_fixture("conv-bg");
    let (executor, mut started, _release) = ControlledExecutor::parking(success());
    let registry = fixture.registry.clone();
    let prepared = registry
        .prepare_dispatch(
            &background_invocation("bash"),
            &(Arc::new(executor) as Arc<dyn rustx::tools::executor::ToolExecutor>),
            rustx::tools::environment::ToolEnvironment::new(),
            None,
        )
        .expect("prepare");
    let outcome = registry
        .commit_dispatch(prepared, &rustx::runtime::CancellationSignal::new())
        .expect("dispatch commits");
    let BackgroundDispatchOutcome::Accepted { execution_id, .. } = outcome else {
        panic!("accepted");
    };
    await_background_started(&mut started, "runner started").await;
    let cancelling = registry.cancel(&execution_id).expect("cancel");
    assert_eq!(cancelling.state, BackgroundLifecycle::Cancelling);
    // The runner observes its background cancellation and settles.
    let settled = wait_for_state(&registry, &execution_id, BackgroundLifecycle::Cancelled).await;
    assert_eq!(settled.state, BackgroundLifecycle::Cancelled);
    let batch = fixture
        .mailbox
        .select_pending_batch()
        .expect("select")
        .expect("one batch");
    assert_eq!(batch.items().len(), 1);
}

/// Repeated cancel is idempotent and never destructive.
#[tokio::test]
async fn repeated_cancel_is_idempotent() {
    let fixture = background_fixture("conv-bg");
    let (executor, mut started, _release) = ControlledExecutor::parking(success());
    let registry = fixture.registry.clone();
    let prepared = registry
        .prepare_dispatch(
            &background_invocation("bash"),
            &(Arc::new(executor) as Arc<dyn rustx::tools::executor::ToolExecutor>),
            rustx::tools::environment::ToolEnvironment::new(),
            None,
        )
        .expect("prepare");
    let outcome = registry
        .commit_dispatch(prepared, &rustx::runtime::CancellationSignal::new())
        .expect("dispatch commits");
    let BackgroundDispatchOutcome::Accepted { execution_id, .. } = outcome else {
        panic!("accepted");
    };
    await_background_started(&mut started, "runner started").await;
    let first = registry.cancel(&execution_id).expect("first cancel");
    assert_eq!(first.state, BackgroundLifecycle::Cancelling);
    let second = registry.cancel(&execution_id).expect("second cancel");
    assert_eq!(second.state, BackgroundLifecycle::Cancelling);
    let third = registry.cancel(&execution_id).expect("third cancel");
    assert_eq!(third.state, BackgroundLifecycle::Cancelling);
    wait_for_state(&registry, &execution_id, BackgroundLifecycle::Cancelled).await;
    let terminal = registry.cancel(&execution_id).expect("cancel terminal");
    assert_eq!(terminal.state, BackgroundLifecycle::Cancelled);
}

/// Starting executions can be cancelled.
#[tokio::test]
async fn starting_can_be_cancelled() {
    let fixture = background_fixture("conv-bg");
    let (executor, mut started, _release) = ControlledExecutor::parking(success());
    let registry = fixture.registry.clone();
    let prepared = registry
        .prepare_dispatch(
            &background_invocation("bash"),
            &(Arc::new(executor) as Arc<dyn rustx::tools::executor::ToolExecutor>),
            rustx::tools::environment::ToolEnvironment::new(),
            None,
        )
        .expect("prepare");
    let outcome = registry
        .commit_dispatch(prepared, &rustx::runtime::CancellationSignal::new())
        .expect("dispatch commits");
    let BackgroundDispatchOutcome::Accepted { execution_id, .. } = outcome else {
        panic!("accepted");
    };
    // Cancel immediately after commit, before the runner begins.
    let snapshot = registry.cancel(&execution_id).expect("cancel");
    assert_eq!(snapshot.state, BackgroundLifecycle::Cancelling);
    await_background_started(&mut started, "runner still starts").await;
    wait_for_state(&registry, &execution_id, BackgroundLifecycle::Cancelled).await;
}

/// Exactly one terminal transition and exactly one terminal mailbox
/// publication, even under duplicate settlement calls.
#[tokio::test]
async fn one_terminal_transition_and_one_publication_only() {
    let fixture = background_fixture("conv-bg");
    let execution_id = dispatch_to_terminal(&fixture).await;
    let terminal = fixture.registry.snapshot(&execution_id).expect("snapshot");
    assert_eq!(terminal.state, BackgroundLifecycle::Succeeded);
    // Duplicate settlement is an idempotent no-op.
    fixture.registry.finish(&execution_id, &success());
    fixture.registry.finish(&execution_id, &cancelled());
    let batch = fixture
        .mailbox
        .select_pending_batch()
        .expect("select")
        .expect("one batch");
    assert_eq!(batch.items().len(), 1);
    assert_eq!(
        batch.items()[0].message().id.as_str(),
        "background-exec_1-terminal"
    );
    let _ = fixture.mailbox.adopt_pending_batch(&batch).expect("adopt");
    assert!(
        fixture
            .mailbox
            .select_pending_batch()
            .expect("select")
            .is_none(),
        "no second publication under duplicate settlement"
    );
}

/// Background progress updates the registry's latest bounded snapshot and
/// emits the canonical execution fact through the narrow event seam.
#[tokio::test]
async fn background_progress_updates_the_latest_snapshot() {
    let fixture = background_fixture("conv-bg");
    let (executor, mut started, release) = ControlledExecutor::parking(success());
    let executor = executor.with_progress(vec![
        ToolProgress {
            message: Some("compiling workspace".to_owned()),
            completed: None,
            total: None,
        },
        ToolProgress {
            message: Some("linking".to_owned()),
            completed: Some(2.0),
            total: Some(3.0),
        },
    ]);
    let registry = fixture.registry.clone();
    let prepared = registry
        .prepare_dispatch(
            &background_invocation("bash"),
            &(Arc::new(executor) as Arc<dyn rustx::tools::executor::ToolExecutor>),
            rustx::tools::environment::ToolEnvironment::new(),
            None,
        )
        .expect("prepare");
    let outcome = registry
        .commit_dispatch(prepared, &rustx::runtime::CancellationSignal::new())
        .expect("dispatch commits");
    let BackgroundDispatchOutcome::Accepted { execution_id, .. } = outcome else {
        panic!("accepted");
    };
    await_background_started(&mut started, "runner started").await;
    let snapshot = registry.snapshot(&execution_id).expect("snapshot");
    assert_eq!(
        snapshot.progress,
        Some(ToolProgress {
            message: Some("linking".to_owned()),
            completed: Some(2.0),
            total: Some(3.0),
        }),
        "only the latest bounded progress snapshot is retained"
    );
    let progress_events = fixture
        .sink
        .as_ref()
        .expect("sink attached")
        .events()
        .into_iter()
        .filter(|event| matches!(event, RuntimeEvent::ToolExecutionProgress { .. }))
        .collect::<Vec<_>>();
    assert_eq!(progress_events.len(), 2);
    assert!(matches!(
        &progress_events[0],
        RuntimeEvent::ToolExecutionProgress {
            tool_call_id,
            tool_id,
            execution_id: Some(reported),
            progress,
        } if tool_call_id.as_str() == "call-1"
            && tool_id.as_str() == "tool-bash"
            && reported.as_str() == "exec_1"
            && progress.message.as_deref() == Some("compiling workspace")
    ));
    release.send_replace(true);
    wait_for_state(&registry, &execution_id, BackgroundLifecycle::Succeeded).await;
}

/// Terminal records remain queryable for the conversation lifetime.
#[tokio::test]
async fn terminal_records_remain_queryable() {
    let fixture = background_fixture("conv-bg");
    let execution_id = dispatch_to_terminal(&fixture).await;
    for _ in 0..3 {
        let snapshot = fixture
            .registry
            .snapshot(&execution_id)
            .expect("terminal record queryable");
        assert_eq!(snapshot.state, BackgroundLifecycle::Succeeded);
        assert_eq!(snapshot.result, Some(success()));
    }
}

/// Another conversation's registry cannot inspect or cancel the execution
/// id: cross-conversation access is structurally impossible.
#[tokio::test]
async fn cross_conversation_isolation() {
    let fixture_a = background_fixture("conv-a");
    let fixture_b = background_fixture("conv-b");
    let execution_id = dispatch_to_terminal(&fixture_a).await;
    assert!(
        fixture_b.registry.snapshot(&execution_id).is_none(),
        "another conversation cannot inspect the id"
    );
    assert!(
        fixture_b.registry.cancel(&execution_id).is_none(),
        "another conversation cannot cancel the id"
    );
    assert_eq!(
        fixture_b.registry.all_snapshots().len(),
        0,
        "no records leak across conversations"
    );
}

async fn wait_for_state(
    registry: &ConversationBackgroundRegistry,
    execution_id: &ToolExecutionId,
    state: BackgroundLifecycle,
) -> BackgroundExecutionSnapshot {
    // Polls the authoritative registry state itself (the very state under
    // test) with a strict deadlock guard.
    for _ in 0..400 {
        let snapshot = registry.snapshot(execution_id).expect("snapshot");
        if snapshot.state == state {
            return snapshot;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let snapshot = registry.snapshot(execution_id).expect("snapshot");
    panic!("state {state:?} never reached; last snapshot: {snapshot:?}");
}

async fn await_background_started(
    started: &mut tokio::sync::watch::Receiver<bool>,
    description: &'static str,
) {
    tokio::time::timeout(
        Duration::from_secs(120),
        started.wait_for(|is_started| *is_started),
    )
    .await
    .unwrap_or_else(|_| panic!("{description}: start wait exceeded liveness guard"))
    .expect("background start channel stays open");
}

// ---------------------------------------------------------------------------
// Loop integration: background dispatch, terminal inbound, Agent Status
// ---------------------------------------------------------------------------

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

fn started() -> ModelEvent {
    ModelEvent::Started
}

fn done(reason: ModelFinishReason) -> ModelEvent {
    ModelEvent::Completed {
        finish_reason: reason,
        usage: None,
    }
}

fn scripted(id: &str, tool_id: &str, name: &str, arguments: serde_json::Value) -> ScriptedCall {
    ScriptedCall {
        id: Box::leak(id.to_owned().into_boxed_str()),
        tool_id: Box::leak(tool_id.to_owned().into_boxed_str()),
        name: Box::leak(name.to_owned().into_boxed_str()),
        arguments,
    }
}

/// Runs the loop over the given tool registry and tool runtime; the runtime
/// owns the canonical conversation mailbox the loop drains.
async fn run_with_mailbox(
    model: &std::sync::Arc<FakeModel>,
    tools: ToolRegistry,
    cancellation: &AgentCancellation,
    tool_runtime: &ConversationToolRuntime,
) -> common::DurableExecutionAudit {
    let store = tool_runtime.durable_store();
    let capability = common::capability_lease(tools, tool_runtime).await;
    let result = AgentExecution::new(
        request(model),
        capability.into_lease(),
        cancellation,
        runtime(model),
        tool_runtime,
        rustx::agent::AttemptLifecycle::inert(),
    )
    .expect("conversation identity matches the tool runtime")
    .run()
    .await;
    common::durable_agent_result(result, store.as_ref())
}

fn tool_messages(result: &AgentExecutionResult) -> Vec<&ToolMessageBlock> {
    result
        .messages()
        .iter()
        .filter_map(|message| match message {
            MessageBlock::Tool(tool) => Some(tool),
            _ => None,
        })
        .collect()
}

/// Background completion after the originating attempt settled does not
/// alter that attempt: the attempt result is frozen and the terminal
/// notification lands in the conversation mailbox.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[allow(clippy::too_many_lines)]
async fn background_completion_after_attempt_terminal_does_not_alter_the_attempt() {
    let dir = tempfile::tempdir().expect("temp dir");
    let workspace_root = dir.path().join("workspace");
    std::fs::create_dir_all(&workspace_root).expect("workspace");
    let tool_runtime = ConversationToolRuntime::new(
        ConversationId::new("conv-1"),
        &workspace_root,
        dir.path().join("artifacts"),
    )
    .expect("tool runtime");
    let mailbox = tool_runtime.mailbox().clone();

    let call = scripted(
        "call-1",
        "tool-bg",
        "bg",
        serde_json::json!({"__rustx_execution": "background"}),
    );
    let model = fake_model(vec![
        vec![
            FakeStep::Emit(started()),
            FakeStep::Emit(tool_call_events(0, &call)[0].clone()),
            FakeStep::Emit(tool_call_events(0, &call)[1].clone()),
            FakeStep::Emit(tool_call_events(0, &call)[2].clone()),
            FakeStep::Emit(done(ModelFinishReason::ToolCalls)),
        ],
        vec![
            FakeStep::Emit(started()),
            FakeStep::Emit(ModelEvent::TextDelta {
                block_index: rustx::message::types::ContentBlockIndex::new(0),
                text: "done".to_owned(),
            }),
            FakeStep::Emit(done(ModelFinishReason::Stop)),
        ],
    ]);
    // The background executor parks until the test releases it; the
    // attempt must never wait for the detached terminal.
    let (tool, release_bg) = FakeTool::parking(
        common::tool_policies(
            "bg",
            "tool-bg",
            ToolExecutionPolicy::ModelSelectable,
            ToolConcurrencyPolicy::Sequential,
        ),
        success_result("bg"),
    );
    let mut tools = ToolRegistry::new();
    tool.register(&mut tools);
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let result = tokio::time::timeout(
        Duration::from_secs(10),
        run_with_mailbox(&model, tools, &cancellation, &tool_runtime),
    )
    .await
    .expect("attempt terminates without the detached terminal");

    assert!(matches!(
        result.outcome,
        AttemptOutcome::Completed {
            finish_reason: ModelFinishReason::Stop
        }
    ));
    let messages = tool_messages(&result);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].result.status, ToolExecutionStatus::Success);
    let accepted = match &messages[0].result.content[0] {
        ToolResultContent::Json { value } => value.clone(),
        other => panic!("expected JSON, got {other:?}"),
    };
    assert_eq!(accepted["execution_id"], "exec_1");
    let committed_count = result.messages().len();
    let terminal_events = result
        .event_history
        .iter()
        .filter(|event| matches!(event, RuntimeEvent::AttemptCompleted { .. }))
        .count();

    // The detached execution settles only after the attempt settled; the
    // terminal notification lands in the conversation mailbox and the
    // settled attempt is never altered.
    let execution_id = ToolExecutionId::new("exec_1");
    release_bg.send_replace(true);
    let settled = wait_for_state(
        tool_runtime.background(),
        &execution_id,
        BackgroundLifecycle::Succeeded,
    )
    .await;
    assert_eq!(settled.state, BackgroundLifecycle::Succeeded);
    let batch = mailbox
        .select_pending_batch()
        .expect("select")
        .expect("terminal batch");
    assert_eq!(batch.items().len(), 1);
    assert_eq!(
        batch.items()[0].message().id.as_str(),
        "background-exec_1-terminal"
    );
    assert_eq!(
        result.messages().len(),
        committed_count,
        "the settled attempt is never altered by the background completion"
    );
    assert_eq!(
        result
            .event_history
            .iter()
            .filter(|event| { matches!(event, RuntimeEvent::AttemptCompleted { .. }) })
            .count(),
        terminal_events,
        "no second terminal event is added to the settled attempt"
    );
}

/// Terminal inbound before the safe-boundary snapshot joins that batch: a
/// foreground parking call holds the batch open while the detached runner
/// completes, so the drain observes the terminal message.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[allow(clippy::too_many_lines)]
async fn terminal_inbound_before_snapshot_joins_the_batch() {
    let dir = tempfile::tempdir().expect("temp dir");
    let workspace_root = dir.path().join("workspace");
    std::fs::create_dir_all(&workspace_root).expect("workspace");
    let tool_runtime = ConversationToolRuntime::new(
        ConversationId::new("conv-1"),
        &workspace_root,
        dir.path().join("artifacts"),
    )
    .expect("tool runtime");
    let call_fg = scripted("call-fg", "tool-fg", "fg", serde_json::json!({}));
    let call_bg = scripted(
        "call-bg",
        "tool-bg",
        "bg",
        serde_json::json!({"__rustx_execution": "background"}),
    );
    let model = fake_model(vec![
        vec![
            FakeStep::Emit(started()),
            FakeStep::Emit(tool_call_events(0, &call_fg)[0].clone()),
            FakeStep::Emit(tool_call_events(0, &call_fg)[1].clone()),
            FakeStep::Emit(tool_call_events(0, &call_fg)[2].clone()),
            FakeStep::Emit(tool_call_events(1, &call_bg)[0].clone()),
            FakeStep::Emit(tool_call_events(1, &call_bg)[1].clone()),
            FakeStep::Emit(tool_call_events(1, &call_bg)[2].clone()),
            FakeStep::Emit(done(ModelFinishReason::ToolCalls)),
        ],
        vec![
            FakeStep::Emit(started()),
            FakeStep::Emit(ModelEvent::TextDelta {
                block_index: rustx::message::types::ContentBlockIndex::new(0),
                text: "done".to_owned(),
            }),
            FakeStep::Emit(done(ModelFinishReason::Stop)),
        ],
    ]);
    let (tool_fg, release_fg) = FakeTool::parking(
        common::tool_policies(
            "fg",
            "tool-fg",
            ToolExecutionPolicy::ForegroundOnly,
            ToolConcurrencyPolicy::Parallel,
        ),
        success_result("fg"),
    );
    let (tool_bg, release_bg) = FakeTool::parking(
        common::tool_policies(
            "bg",
            "tool-bg",
            ToolExecutionPolicy::ModelSelectable,
            ToolConcurrencyPolicy::Parallel,
        ),
        success_result("bg"),
    );
    let mut bg_started = tool_bg.started();
    let mut tools = ToolRegistry::new();
    tool_fg.register(&mut tools);
    tool_bg.register(&mut tools);
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);

    let controller_registry = tool_runtime.background().clone();
    let controller = tokio::spawn(async move {
        // The detached runner is running; release it while the foreground
        // call still parks. The terminal state transition and its inbound
        // enqueue share one registry critical section, so observing the
        // terminal registry state deterministically means the enqueue
        // already committed before the safe-boundary snapshot of this turn.
        await_background_started(&mut bg_started, "bg started").await;
        release_bg.send_replace(true);
        let execution_id = ToolExecutionId::new("exec_1");
        wait_for_state(
            &controller_registry,
            &execution_id,
            BackgroundLifecycle::Succeeded,
        )
        .await;
        release_fg.send_replace(true);
    });
    let result = tokio::time::timeout(
        Duration::from_secs(10),
        run_with_mailbox(&model, tools, &cancellation, &tool_runtime),
    )
    .await
    .expect("run terminates");
    controller.await.expect("controller task");

    // The terminal inbound arrived before the safe-boundary snapshot and
    // joined the drained batch: the continuation request carries it as a
    // canonical user message.
    let requests = model.requests();
    assert_eq!(requests.len(), 2);
    let inbound_in_continuation = requests[1]
        .messages
        .iter()
        .any(|message| matches!(message, MessageBlock::User(user) if user.id.as_str() == "background-exec_1-terminal"));
    assert!(
        inbound_in_continuation,
        "the terminal inbound joined the drained batch of the tool turn"
    );
    assert!(matches!(
        result.outcome,
        AttemptOutcome::Completed {
            finish_reason: ModelFinishReason::Stop
        }
    ));
}

// ---------------------------------------------------------------------------
// background_task intrinsic
// ---------------------------------------------------------------------------

/// The foreground-only `background_task` intrinsic reports the canonical
/// snapshot and supports idempotent cancel.
#[tokio::test]
async fn background_task_status_and_cancel() {
    let fixture = common::native_fixture();
    // Dispatch one parking background execution through the intrinsic's own
    // registry.
    let (executor, mut started, _release) = ControlledExecutor::parking(success());
    let registry = fixture.runtime.background().clone();
    let prepared = registry
        .prepare_dispatch(
            &background_invocation("bash"),
            &(Arc::new(executor) as Arc<dyn rustx::tools::executor::ToolExecutor>),
            rustx::tools::environment::ToolEnvironment::new(),
            None,
        )
        .expect("prepare");
    let outcome = registry
        .commit_dispatch(prepared, &rustx::runtime::CancellationSignal::new())
        .expect("dispatch commits");
    let BackgroundDispatchOutcome::Accepted { .. } = outcome else {
        panic!("accepted");
    };
    await_background_started(&mut started, "runner started").await;

    let status = common::run_tool(
        &fixture,
        "background_task",
        serde_json::json!({"execution_id": "exec_1", "action": "status"}),
    )
    .await;
    assert_eq!(status.status, ToolExecutionStatus::Success);
    let snapshot = match &status.content[0] {
        ToolResultContent::Json { value } => value.clone(),
        other => panic!("expected JSON, got {other:?}"),
    };
    assert_eq!(snapshot["execution_id"], "exec_1");
    assert_eq!(snapshot["tool_name"], "bash");
    assert_eq!(snapshot["state"], "running");

    let cancelled = common::run_tool(
        &fixture,
        "background_task",
        serde_json::json!({"execution_id": "exec_1", "action": "cancel"}),
    )
    .await;
    assert_eq!(cancelled.status, ToolExecutionStatus::Success);
    let snapshot = match &cancelled.content[0] {
        ToolResultContent::Json { value } => value.clone(),
        other => panic!("expected JSON, got {other:?}"),
    };
    assert_eq!(snapshot["state"], "cancelling");
    // Repeated cancel is idempotent.
    let again = common::run_tool(
        &fixture,
        "background_task",
        serde_json::json!({"execution_id": "exec_1", "action": "cancel"}),
    )
    .await;
    assert_eq!(again.status, ToolExecutionStatus::Success);
    let execution_id = ToolExecutionId::new("exec_1");
    wait_for_state(&registry, &execution_id, BackgroundLifecycle::Cancelled).await;
    let terminal = common::run_tool(
        &fixture,
        "background_task",
        serde_json::json!({"execution_id": "exec_1", "action": "cancel"}),
    )
    .await;
    let snapshot = match &terminal.content[0] {
        ToolResultContent::Json { value } => value.clone(),
        other => panic!("expected JSON, got {other:?}"),
    };
    assert_eq!(
        snapshot["state"], "cancelled",
        "cancel of a terminal is a no-op"
    );
}

/// Unknown execution ids (including another conversation's ids, which are
/// indistinguishable) return a normal failed tool result.
#[tokio::test]
async fn background_task_unknown_and_foreign_ids_fail_normally() {
    let fixture = common::native_fixture();
    let unknown = common::run_tool(
        &fixture,
        "background_task",
        serde_json::json!({"execution_id": "exec_999", "action": "status"}),
    )
    .await;
    assert!(matches!(unknown.status, ToolExecutionStatus::Failed { .. }));
    let foreign = common::run_tool(
        &fixture,
        "background_task",
        serde_json::json!({"execution_id": "exec_1", "action": "cancel"}),
    )
    .await;
    assert!(matches!(foreign.status, ToolExecutionStatus::Failed { .. }));
}

/// `background_task` is fixed to foreground-only sequential execution: it
/// can never be dispatched to the background registry.
#[test]
fn background_task_is_never_background_dispatchable() {
    let fixture = common::native_fixture();
    let call = ToolCall {
        id: ToolCallId::new("call-x"),
        tool_id: ToolId::new("tool-background-task"),
        name: "background_task".to_owned(),
        arguments: serde_json::json!({"__rustx_execution": "background", "execution_id": "exec_1", "action": "status"}),
    };
    let outcome = fixture.registry.preflight(&call).expect("preflight");
    assert!(
        matches!(
            outcome,
            rustx::tools::executor::PreflightOutcome::Rejected { .. }
        ),
        "the fixed foreground-only intrinsic rejects background invocation metadata"
    );
}

// ---------------------------------------------------------------------------
// Agent Status background built-in section
// ---------------------------------------------------------------------------

/// The composer builds the runtime-owned background section in deterministic
/// section order; the renderer shows ids, tool names, and states without
/// full output.
#[test]
fn agent_status_background_section_rendering() {
    use rustx::context::AgentStatusClock;
    use rustx::context::status::{
        AgentStatusComposer, AgentStatusRenderContext, AgentStatusSectionData, AgentStatusSectionId,
    };
    struct FixedClock(chrono::DateTime<chrono::Utc>);
    impl AgentStatusClock for FixedClock {
        fn now(&self) -> chrono::DateTime<chrono::Utc> {
            self.0
        }
    }
    let composer = AgentStatusComposer::new(Arc::new(FixedClock(utc("2026-08-09T12:00:00Z"))));
    let background = vec![
        BackgroundExecutionSnapshot {
            execution_id: ToolExecutionId::new("exec_1"),
            tool_id: ToolId::new("tool-bash"),
            tool_name: "bash".to_owned(),
            state: BackgroundLifecycle::Starting,
            progress: None,
            result: None,
        },
        BackgroundExecutionSnapshot {
            execution_id: ToolExecutionId::new("exec_2"),
            tool_id: ToolId::new("tool-bash"),
            tool_name: "bash".to_owned(),
            state: BackgroundLifecycle::Running,
            progress: Some(ToolProgress {
                message: Some("compiling workspace".to_owned()),
                completed: None,
                total: None,
            }),
            result: None,
        },
        BackgroundExecutionSnapshot {
            execution_id: ToolExecutionId::new("exec_3"),
            tool_id: ToolId::new("tool-grep"),
            tool_name: "grep".to_owned(),
            state: BackgroundLifecycle::Cancelling,
            progress: None,
            result: None,
        },
    ];
    let context = AgentStatusRenderContext {
        inbound_message_time: utc("2026-08-09T12:00:00Z"),
        timezone: None,
        background,
    };
    let status = composer.compose(&context).expect("compose");
    let ids: Vec<&str> = status
        .sections
        .iter()
        .map(|section| section.id.as_str())
        .collect();
    assert_eq!(ids, vec!["temporal", "background_execution"]);
    let section = &status.sections[1];
    assert_eq!(
        section.id,
        AgentStatusSectionId::new("background_execution")
    );
    let AgentStatusSectionData::BackgroundExecution { executions } = &section.data else {
        panic!("runtime-owned built-in section data");
    };
    assert_eq!(executions.len(), 3);
    let rendered = rustx::context::status::render_agent_status(&status);
    assert!(rendered.contains("Background executions:"));
    assert!(rendered.contains("- exec_1 | bash | starting"));
    assert!(rendered.contains("- exec_2 | bash | running | compiling workspace"));
    assert!(rendered.contains("- exec_3 | grep | cancelling"));
    // Full output never appears: the rendered status carries identities and
    // states only.
    assert!(!rendered.contains("BackgroundExecutionSnapshot"));
}

/// Only active executions appear in the active snapshot used by Agent
/// Status, in execution-allocation order.
#[tokio::test]
async fn agent_status_active_snapshot_excludes_terminal_entries() {
    let fixture = background_fixture("conv-bg");
    let first = dispatch_to_terminal(&fixture).await;
    let _ = first;
    let (executor, mut started, _release) = ControlledExecutor::parking(success());
    let registry = fixture.registry.clone();
    let prepared = registry
        .prepare_dispatch(
            &background_invocation("grep"),
            &(Arc::new(executor) as Arc<dyn rustx::tools::executor::ToolExecutor>),
            rustx::tools::environment::ToolEnvironment::new(),
            None,
        )
        .expect("prepare");
    let outcome = registry
        .commit_dispatch(prepared, &rustx::runtime::CancellationSignal::new())
        .expect("dispatch commits");
    let BackgroundDispatchOutcome::Accepted { execution_id, .. } = outcome else {
        panic!("accepted");
    };
    await_background_started(&mut started, "runner started").await;
    let active = registry.active_snapshot();
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].execution_id, execution_id);
    assert_eq!(active[0].tool_name, "grep");
    assert_eq!(active[0].state, BackgroundLifecycle::Running);
}

/// A fresh terminal runtime inbound turn carries an Agent Status snapshot
/// showing the remaining active tasks only.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(clippy::too_many_lines)]
async fn fresh_terminal_inbound_status_shows_remaining_active_tasks() {
    let dir = tempfile::tempdir().expect("temp dir");
    let workspace_root = dir.path().join("workspace");
    std::fs::create_dir_all(&workspace_root).expect("workspace");
    let tool_runtime = ConversationToolRuntime::new(
        ConversationId::new("conv-1"),
        &workspace_root,
        dir.path().join("artifacts"),
    )
    .expect("tool runtime");
    let call_b1 = scripted(
        "call-b1",
        "tool-b1",
        "b1",
        serde_json::json!({"__rustx_execution": "background"}),
    );
    let call_b2 = scripted(
        "call-b2",
        "tool-b2",
        "b2",
        serde_json::json!({"__rustx_execution": "background"}),
    );
    let (model_release_tx, model_release_rx) = support::fake::model_release();
    let model = fake_model(vec![
        vec![
            FakeStep::Emit(started()),
            FakeStep::Emit(tool_call_events(0, &call_b1)[0].clone()),
            FakeStep::Emit(tool_call_events(0, &call_b1)[1].clone()),
            FakeStep::Emit(tool_call_events(0, &call_b1)[2].clone()),
            FakeStep::Emit(tool_call_events(1, &call_b2)[0].clone()),
            FakeStep::Emit(tool_call_events(1, &call_b2)[1].clone()),
            FakeStep::Emit(tool_call_events(1, &call_b2)[2].clone()),
            FakeStep::Emit(done(ModelFinishReason::ToolCalls)),
        ],
        vec![
            FakeStep::Emit(started()),
            // The second model generation parks while the detached B1
            // completes, so the terminal inbound provably enqueues after
            // the first safe-boundary snapshot.
            FakeStep::ParkUntilReleased(model_release_rx),
            FakeStep::Emit(ModelEvent::TextDelta {
                block_index: rustx::message::types::ContentBlockIndex::new(0),
                text: "turn two".to_owned(),
            }),
            FakeStep::Emit(done(ModelFinishReason::Stop)),
        ],
        vec![
            FakeStep::Emit(started()),
            FakeStep::Emit(ModelEvent::TextDelta {
                block_index: rustx::message::types::ContentBlockIndex::new(0),
                text: "turn three".to_owned(),
            }),
            FakeStep::Emit(done(ModelFinishReason::Stop)),
        ],
    ]);
    let (tool_b1, release_b1) = FakeTool::parking(
        common::tool_policies(
            "b1",
            "tool-b1",
            ToolExecutionPolicy::ModelSelectable,
            ToolConcurrencyPolicy::Parallel,
        ),
        success_result("b1"),
    );
    let (tool_b2, _never_released) = FakeTool::parking(
        common::tool_policies(
            "b2",
            "tool-b2",
            ToolExecutionPolicy::ModelSelectable,
            ToolConcurrencyPolicy::Parallel,
        ),
        success_result("b2"),
    );
    let mut started_b1 = tool_b1.started();
    let mut started_b2 = tool_b2.started();
    let mut model_parked = model.parked();
    let mut tools = ToolRegistry::new();
    tool_b1.register(&mut tools);
    tool_b2.register(&mut tools);
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);

    let controller_registry = tool_runtime.background().clone();
    let controller = tokio::spawn(async move {
        // Wait for the second model generation to park: that happens only
        // after the first turn's safe-boundary selection already committed,
        // so the terminal inbound below provably enqueues after that
        // snapshot (and can never join the first batch).
        model_parked
            .wait_for(|is_parked| *is_parked)
            .await
            .expect("the second model generation parks");
        await_background_started(&mut started_b1, "b1 started").await;
        await_background_started(&mut started_b2, "b2 started").await;
        // B1 settles while the second model generation is parked. The
        // terminal registry transition and its inbound enqueue share one
        // registry critical section, so observing the terminal registry
        // state deterministically means the enqueue already committed
        // before the model is released — after the first safe-boundary
        // snapshot, which B1 could not have reached while parked.
        release_b1.send_replace(true);
        wait_for_state(
            &controller_registry,
            &ToolExecutionId::new("exec_1"),
            BackgroundLifecycle::Succeeded,
        )
        .await;
        model_release_tx.send(true).expect("release the model");
    });
    let result = tokio::time::timeout(
        Duration::from_secs(15),
        run_with_mailbox(&model, tools, &cancellation, &tool_runtime),
    )
    .await
    .expect("run terminates");
    controller.await.expect("controller task");

    let requests = model.requests();
    assert_eq!(requests.len(), 3);
    // Request 2 is a foreground tool-result continuation: no Agent Status.
    assert!(!requests[1].messages.iter().any(|message| {
        matches!(
            message,
            MessageBlock::User(user)
                if user.kind
                    == rustx::message::types::InboundKind::Context(
                        rustx::message::types::ContextKind::AgentStatus,
                    )
        )
    }));
    // Request 3 is the fresh terminal inbound turn: its canonical Agent
    // Status fact shows
    // only the remaining active task.
    let status = requests[2]
        .messages
        .iter()
        .find_map(|message| match message {
            MessageBlock::User(user)
                if user.kind
                    == rustx::message::types::InboundKind::Context(
                        rustx::message::types::ContextKind::AgentStatus,
                    ) =>
            {
                user.content.first().and_then(|content| match content {
                    rustx::message::types::UserContentBlock::Text(text) => Some(text.text.clone()),
                    _ => None,
                })
            }
            _ => None,
        })
        .expect("fresh terminal inbound carries Agent Status");
    assert!(
        status.contains("exec_2"),
        "the remaining active task appears"
    );
    assert!(!status.contains("exec_1"), "the terminal task is excluded");
    assert!(
        requests[2]
            .messages
            .iter()
            .any(|message| matches!(message, MessageBlock::User(user) if user.id.as_str() == "background-exec_1-terminal")),
        "the terminal inbound waited for the next batch"
    );
    assert!(matches!(
        result.outcome,
        AttemptOutcome::Completed {
            finish_reason: ModelFinishReason::Stop
        }
    ));
}

/// A foreground tool-result continuation carries no Agent Status.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn foreground_tool_continuation_has_no_agent_status() {
    let dir = tempfile::tempdir().expect("temp dir");
    let workspace_root = dir.path().join("workspace");
    std::fs::create_dir_all(&workspace_root).expect("workspace");
    let tool_runtime = ConversationToolRuntime::new(
        ConversationId::new("conv-1"),
        &workspace_root,
        dir.path().join("artifacts"),
    )
    .expect("tool runtime");
    let call = scripted("call-1", "tool-alpha", "alpha", serde_json::json!({}));
    let model = fake_model(vec![
        vec![
            FakeStep::Emit(started()),
            FakeStep::Emit(tool_call_events(0, &call)[0].clone()),
            FakeStep::Emit(tool_call_events(0, &call)[1].clone()),
            FakeStep::Emit(tool_call_events(0, &call)[2].clone()),
            FakeStep::Emit(done(ModelFinishReason::ToolCalls)),
        ],
        vec![
            FakeStep::Emit(started()),
            FakeStep::Emit(ModelEvent::TextDelta {
                block_index: rustx::message::types::ContentBlockIndex::new(0),
                text: "done".to_owned(),
            }),
            FakeStep::Emit(done(ModelFinishReason::Stop)),
        ],
    ]);
    let tool = FakeTool::new(
        common::tool_policies(
            "alpha",
            "tool-alpha",
            ToolExecutionPolicy::ForegroundOnly,
            ToolConcurrencyPolicy::Sequential,
        ),
        success_result("ok"),
    );
    let mut tools = ToolRegistry::new();
    tool.register(&mut tools);
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let _result = tokio::time::timeout(
        Duration::from_secs(10),
        run_with_mailbox(&model, tools, &cancellation, &tool_runtime),
    )
    .await
    .expect("run terminates");
    let requests = model.requests();
    assert_eq!(requests.len(), 2);
    assert!(
        !requests[1].messages.iter().any(|message| {
            matches!(
                message,
                MessageBlock::User(user)
                    if user.kind
                        == rustx::message::types::InboundKind::Context(
                            rustx::message::types::ContextKind::AgentStatus,
                        )
            )
        }),
        "foreground tool-result continuation carries no Agent Status"
    );
}

/// The background section participates in the projection fingerprint and
/// the full request token estimate, and never satisfies `keep_recent_tokens`.
#[test]
fn background_status_accounting() {
    use rustx::context::engine::ContextConfig;
    use rustx::context::status::{
        AgentStatus, AgentStatusSection, AgentStatusSectionData, AgentStatusSectionId,
        render_agent_status,
    };
    use rustx::context::tokens::DefaultTokenEstimator;
    use rustx::context::{ContextEngine, TokenEstimator as _};
    let estimator: Arc<dyn rustx::context::TokenEstimator> = Arc::new(DefaultTokenEstimator);
    let engine = ContextEngine::new(
        ContextConfig {
            context_window_tokens: 10_000,
            reserve_tokens: 0,
            keep_recent_tokens: 100,
        },
        estimator,
    )
    .expect("engine");
    let rendered = render_agent_status(&AgentStatus {
        sections: vec![
            AgentStatusSection {
                id: AgentStatusSectionId::new("temporal"),
                data: AgentStatusSectionData::Temporal {
                    current_time: utc("2026-08-09T12:00:00Z"),
                    timezone: None,
                    inbound_message_time: utc("2026-08-09T12:00:00Z"),
                },
            },
            AgentStatusSection {
                id: AgentStatusSectionId::new("background_execution"),
                data: AgentStatusSectionData::BackgroundExecution {
                    executions: vec![BackgroundExecutionSnapshot {
                        execution_id: ToolExecutionId::new("exec_1"),
                        tool_id: ToolId::new("tool-bash"),
                        tool_name: "bash".to_owned(),
                        state: BackgroundLifecycle::Running,
                        progress: None,
                        result: None,
                    }],
                },
            },
        ],
    });
    let empty = rustx::conversation::ConversationState::new();
    let with_status = engine
        .build_projection(&empty, &[], None, &rendered)
        .expect("projection with status");
    let without_status = engine
        .build_projection(&empty, &[], None, "")
        .expect("projection without status");
    assert_ne!(
        with_status.fingerprint(),
        without_status.fingerprint(),
        "the background section changes the projection fingerprint"
    );
    let tools: Vec<rustx::tools::types::ModelToolDefinition> = Vec::new();
    let estimator = DefaultTokenEstimator;
    assert!(
        estimator.estimate_input(
            &with_status.messages,
            &with_status.effective_system_prompt,
            &tools
        ) > estimator.estimate_input(
            &without_status.messages,
            &without_status.effective_system_prompt,
            &tools
        ),
        "the background section participates in the full request estimate"
    );
    assert_eq!(
        estimator.estimate_conversation_input(&with_status.messages),
        estimator.estimate_conversation_input(&without_status.messages),
        "keep_recent_tokens is unaffected by the status size"
    );
    // keep_recent_tokens is measured over conversation content only, and
    // the Effective System Prompt is outside the conversation-estimation
    // boundary entirely: the same ordered messages estimate identically
    // regardless of the request-time prompt.
    assert_eq!(
        estimator.estimate_conversation_input(&[]),
        estimator.estimate_conversation_input(&without_status.messages),
    );
}
