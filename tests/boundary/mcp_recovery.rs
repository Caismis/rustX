//! Issue #205 boundary conformance: MCP tool-call liveness, cancellation,
//! transport loss, reconnection, and last-known-good capability recovery,
//! proven over real MCP stdio child servers.
//!
//! Every scenario here drives the production path end to end: the capability
//! coordinator connects a real child process, publishes a real capability
//! generation, and the Agent Loop's generic Issue #204 lifecycle executes
//! the resulting MCP executor. Nothing is simulated except the clock.
//!
//! # How ordering is proven
//!
//! - **the generic lifecycle's own arming signal** (`ToolDeadlineArmedSignal`)
//!   pins the exact monotonic frontier the hard deadline is measured from, so
//!   the manual clock crosses a value the test asserted rather than a value
//!   it guessed;
//! - **the in-band MCP progress notification** the recovery fixture emits for
//!   every accepted `tools/call` is observed in the parent through the
//!   ordinary progress seam. Receiving it proves the dispatched request
//!   reached the server — strictly stronger than the effect frontier rustX
//!   classifies against — so "the transport died *after* dispatch" is a
//!   channel receive, never a sleep;
//! - **the fixture journal**, shared by every generation of one server
//!   identity, records one line per accepted `tools/call`. Counting those
//!   lines across a reconnection is what proves an ambiguous invocation was
//!   received at most once.
//!
//! Wall-clock time appears only in outer anti-hang guards.

#![cfg(all(unix, feature = "mcp-fixture"))]

use super::{common, support};

use std::sync::Arc;
use std::time::Duration;

use rustx::agent::{
    AgentCancellation, AgentExecution, AgentExecutionObserver, AgentExecutionRequest,
    AgentStatusObservation,
};
use rustx::capabilities::{CapabilityCoordinator, CapabilityCoordinatorConfig};
use rustx::durable::TranscriptCursor;
use rustx::events::types::RuntimeEvent;
use rustx::message::content::TextBlock;
use rustx::message::types::{MessageBlock, UserContentBlock, UserMessageBlock, UserSource};
use rustx::model::event::ModelEvent;
use rustx::model::finish::ModelFinishReason;
use rustx::publication::{PublicationAudit, PublicationFrame, PublicationStreamStart};
use rustx::runtime::identity::{AgentId, AttemptId, McpServerId, MessageId};
use rustx::runtime::types::CancellationReason;
use rustx::runtime::{ManualMonotonicClock, MonotonicClock};
use rustx::tools::deadline::{ToolDeadlineKind, ToolExecutionDeadlinePolicy};
use rustx::tools::mcp::fixture::{recovery, streamable_http};
use rustx::tools::types::ToolExecutionStatus;
use support::fake::{FakeModel, FakeStep, ScriptedCall, fake_model, tool_call_events};
use tokio::sync::watch;

// ---------------------------------------------------------------------------
// Fixture wiring
// ---------------------------------------------------------------------------

/// The binding of one recovery-fixture server: this test binary re-executed
/// as exactly `test_name` in recovery-fixture mode.
fn recovery_binding(
    test_name: &str,
    control: &recovery::RecoveryControl,
    script: &recovery::RecoveryScript,
) -> rustx::tools::mcp::McpServerBinding {
    rustx::tools::mcp::McpServerBinding {
        credentials: rustx::credentials::SourceCredentials::default(),
        activation: rustx::capabilities::activation::SourceActivation::Enabled,
        resource_workspace: None,
        transport: rustx::tools::mcp::McpTransportConfig::Stdio {
            program: std::env::current_exe()
                .expect("test executable")
                .display()
                .to_string(),
            args: rustx::tools::mcp::fixture::fixture_spawn_args(test_name),
            cwd: None,
            environment: control.environment(script),
        },
        policy: rustx::tools::types::ToolInvocationPolicy::default(),
    }
}

/// A published capability generation backed by one real recovery-fixture
/// child, built through the production coordinator path so every executor is
/// bound to a real reconnectable MCP connection.
struct McpCapability {
    coordinator: CapabilityCoordinator,
    server_id: McpServerId,
    _dir: tempfile::TempDir,
}

async fn recovery_capability(
    tool_runtime: &rustx::tools::runtime::ConversationToolRuntime,
    test_name: &str,
    control: &recovery::RecoveryControl,
    script: &recovery::RecoveryScript,
) -> McpCapability {
    let dir = tempfile::tempdir().expect("capability temp dir");
    let server_id = McpServerId::new("recovery");
    let coordinator = CapabilityCoordinator::new(CapabilityCoordinatorConfig {
        python_sources: std::collections::BTreeMap::new(),
        conversation_id: tool_runtime.conversation_id().clone(),
        workspace: tool_runtime.workspace().clone(),
        base_tool_registry: Arc::new(rustx::tools::executor::ToolRegistry::new()),
        extension_tools: tool_runtime.extension_tool_plane(),
        tool_activation: rustx::capabilities::ToolActivationPolicy::default(),
        skill_discovery: rustx::skills::SkillDiscoveryConfig::default(),
        mcp_servers: std::collections::BTreeMap::from([(
            server_id.clone(),
            recovery_binding(test_name, control, script),
        )]),
        base_environment: tool_runtime.environment().clone(),
        environment_store_root: dir.path().join("skill-env"),
    })
    .expect("capability coordinator");
    let candidate = coordinator
        .prepare_candidate()
        .await
        .expect("the recovery fixture publishes its catalog");
    coordinator.commit(candidate).expect("commit generation 1");
    McpCapability {
        coordinator,
        server_id,
        _dir: dir,
    }
}

impl McpCapability {
    fn definition_names(&self) -> Vec<String> {
        self.coordinator
            .current_snapshot()
            .tool_registry()
            .definitions()
            .into_iter()
            .map(|definition| definition.name)
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Agent Loop driver
// ---------------------------------------------------------------------------

/// One observation on the runtime's own publication path, recorded in the
/// order the runtime published it.
///
/// Both variants are written from the runtime's observer callbacks, which is
/// a single defined ordering boundary — never from a test task sampling
/// state — so the position of one relative to the other is a genuine
/// happens-before rather than an inference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OrderedObservation {
    /// The generic progress seam forwarded one MCP progress report.
    Progress,
    /// The canonical terminal `ToolExecutionCompleted` fact was published.
    Completed,
}

/// The ordered log of those observations for one attempt.
#[derive(Default)]
struct OrderedObservations {
    facts: std::sync::Mutex<Vec<OrderedObservation>>,
    /// Installed only for the progress boundedness regression. Sampled on
    /// the terminal publication path, before the next model turn can dispatch.
    progress_runtime: Option<Arc<rustx::tools::mcp::McpServerRuntime>>,
    settled_progress_cardinalities: std::sync::Mutex<Vec<usize>>,
}

impl OrderedObservations {
    fn record(&self, fact: OrderedObservation) {
        if fact == OrderedObservation::Completed
            && let Some(runtime) = &self.progress_runtime
        {
            self.settled_progress_cardinalities
                .lock()
                .expect("settled cardinalities lock")
                .push(runtime.tracked_progress_requests());
        }
        self.facts
            .lock()
            .expect("ordered observation lock")
            .push(fact);
    }

    fn facts(&self) -> Vec<OrderedObservation> {
        self.facts.lock().expect("ordered observation lock").clone()
    }
}

/// Observes the durable execution facts a controller needs: the start fact
/// and every forwarded MCP progress report.
struct ExecutionSignals {
    started: watch::Sender<bool>,
    progress: watch::Sender<u32>,
    /// Installed only by a scenario whose contract is about the order of the
    /// live progress seam relative to the terminal fact.
    ordered: Option<Arc<OrderedObservations>>,
}

impl AgentExecutionObserver for ExecutionSignals {
    fn observe_event(&self, _attempt_id: &AttemptId, event: &RuntimeEvent) {
        if matches!(event, RuntimeEvent::ToolExecutionStarted { .. }) {
            self.started.send_replace(true);
        }
        if matches!(event, RuntimeEvent::ToolExecutionCompleted { .. })
            && let Some(ordered) = &self.ordered
        {
            ordered.record(OrderedObservation::Completed);
        }
    }

    /// The **live** progress seam (Issue #178): the durable
    /// `ToolExecutionProgress` facts commit only at batch settlement, so this
    /// is the only observation available while the tool still executes.
    ///
    /// The progress fanout refreshes the idle-liveness watchdog *before* it
    /// calls this observer, so returning from here is a happens-after of the
    /// corresponding idle refresh. That is what lets a controller advance the
    /// manual clock without racing an unpublished refresh.
    fn observe_tool_progress(
        &self,
        _attempt_id: &AttemptId,
        _tool_call_id: &rustx::runtime::identity::ToolCallId,
        _tool_id: &rustx::runtime::identity::ToolId,
        _progress: &rustx::tools::types::ToolProgress,
    ) {
        if let Some(ordered) = &self.ordered {
            ordered.record(OrderedObservation::Progress);
        }
        self.progress.send_modify(|count| *count += 1);
    }

    fn observe_committed(
        &self,
        _attempt_id: &AttemptId,
        _block: &MessageBlock,
        _transcript_cursor: Option<TranscriptCursor>,
    ) {
    }

    fn observe_status(&self, _observation: &AgentStatusObservation) {}

    fn observe_publication_opened(&self, _attempt_id: &AttemptId, _start: &PublicationStreamStart) {
    }

    fn observe_publication(&self, _attempt_id: &AttemptId, _frame: &PublicationFrame) {}

    fn observe_publication_settled(
        &self,
        _attempt_id: &AttemptId,
        _audit: &PublicationAudit,
        _transcript_cursor: TranscriptCursor,
    ) {
    }
}

/// What a controller may observe while the attempt runs.
struct Controls {
    /// Resolves once the durable `ToolExecutionStarted` fact was observed.
    started: watch::Receiver<bool>,
    /// Counts forwarded MCP progress reports. The recovery fixture emits one
    /// per accepted `tools/call`, so `>= 1` proves the request reached the
    /// server.
    progress: watch::Receiver<u32>,
    /// Publishes the absolute monotonic hard deadline of the running call the
    /// instant the generic lifecycle armed it.
    armed: watch::Receiver<Option<u64>>,
    /// Signals when the physical-completion branch has provably won the
    /// lifecycle's winner arbitration for this call, and releases it again.
    /// Cancellation issued while parked here is provably too late to reclaim
    /// settlement authority.
    physical_won: watch::Receiver<bool>,
    release_physical: std::sync::mpsc::Sender<()>,
    clock: Arc<ManualMonotonicClock>,
    cancellation: AgentCancellation,
}

impl Controls {
    async fn wait_started(&mut self) {
        self.started
            .wait_for(|started| *started)
            .await
            .expect("the tool start observation channel stays open");
    }

    async fn wait_progress_at_least(&mut self, count: u32) {
        self.progress
            .wait_for(|observed| *observed >= count)
            .await
            .expect("the progress observation channel stays open");
    }

    /// Waits until the executor's physical result provably won arbitration.
    async fn wait_physical_won(&mut self) {
        self.physical_won
            .wait_for(|won| *won)
            .await
            .expect("the physical settlement pause channel stays open");
    }

    /// Crosses the armed hard deadline of the running call.
    async fn cross_hard_deadline(&mut self) -> u64 {
        let deadline = (*self
            .armed
            .wait_for(Option::is_some)
            .await
            .expect("the hard deadline is armed while the execution runs"))
        .expect("armed");
        self.clock.advance(deadline);
        deadline
    }
}

/// Runs one attempt that issues the given MCP tool calls — one per model
/// turn, in order — then a plain text turn, and returns the durable audit.
///
/// Sequencing several calls inside **one** attempt is deliberate: every call
/// must observe the same conversation identity and the same durable
/// authority, and a second attempt over the same store would reconstruct a
/// different canonical request. It also matches the real shape of the
/// contract under test, where a later `ToolCall` of the same conversation is
/// served by a replacement connection generation.
async fn run_mcp_calls<C, F>(
    fixture: &common::NativeFixture,
    capability: rustx::capabilities::AttemptCapabilityLease,
    attempt: &str,
    tool_names: &[&str],
    policy: ToolExecutionDeadlinePolicy,
    controller: C,
) -> common::DurableExecutionAudit
where
    C: FnOnce(Controls) -> F + Send + 'static,
    F: std::future::Future<Output = ()> + Send + 'static,
{
    let turns: Vec<Vec<&str>> = tool_names.iter().map(|name| vec![*name]).collect();
    run_mcp_turns(
        fixture, capability, attempt, &turns, policy, None, controller,
    )
    .await
}

/// Runs one attempt over the given model turns: one entry per turn, each
/// listing the MCP tools that turn calls, in order.
async fn run_mcp_turns<C, F>(
    fixture: &common::NativeFixture,
    capability: rustx::capabilities::AttemptCapabilityLease,
    attempt: &str,
    turn_tools: &[Vec<&str>],
    policy: ToolExecutionDeadlinePolicy,
    ordered: Option<Arc<OrderedObservations>>,
    controller: C,
) -> common::DurableExecutionAudit
where
    C: FnOnce(Controls) -> F + Send + 'static,
    F: std::future::Future<Output = ()> + Send + 'static,
{
    let definitions = capability.snapshot().tool_registry().definitions();
    let mut turns = Vec::new();
    for (turn_index, tool_names) in turn_tools.iter().enumerate() {
        let mut turn = vec![FakeStep::Emit(ModelEvent::Started)];
        for (call_index, tool_name) in tool_names.iter().enumerate() {
            let tool_id = definitions
                .iter()
                .find(|definition| definition.name == *tool_name)
                .expect("the published catalog carries the tool")
                .id
                .as_str()
                .to_owned();
            let scripted = ScriptedCall {
                id: Box::leak(format!("call-{attempt}-{turn_index}-{call_index}").into_boxed_str()),
                tool_id: Box::leak(tool_id.into_boxed_str()),
                // Fixture tool names are minted per fixture instance, so
                // they are owned rather than literal; the scripted call
                // shape wants `'static`, exactly as for the two ids above.
                name: Box::leak(String::from(*tool_name).into_boxed_str()),
                arguments: serde_json::json!({}),
            };
            for event in tool_call_events(
                u32::try_from(call_index).expect("small parallel batch"),
                &scripted,
            ) {
                turn.push(FakeStep::Emit(event));
            }
        }
        turn.push(FakeStep::Emit(ModelEvent::Completed {
            finish_reason: ModelFinishReason::ToolCalls,
            usage: None,
        }));
        turns.push(turn);
    }
    turns.push(vec![
        FakeStep::Emit(ModelEvent::Started),
        FakeStep::Emit(ModelEvent::TextDelta {
            block_index: rustx::message::types::ContentBlockIndex::new(0),
            text: "done".to_owned(),
        }),
        FakeStep::Emit(ModelEvent::Completed {
            finish_reason: ModelFinishReason::Stop,
            usage: None,
        }),
    ]);
    let model: Arc<FakeModel> = fake_model(turns);

    let clock = Arc::new(ManualMonotonicClock::new());
    let snapshot = support::attempt_model(model.clone(), "fake-model");
    let context_runtime = rustx::context::ContextRuntime::for_attempt(
        rustx::context::SessionContextPolicy {
            reserve_tokens: 0,
            keep_recent_tokens: 0,
            summary_output_cap: None,
        },
        Arc::new(rustx::context::DefaultTokenEstimator),
        Some(rustx::context::AgentStatusEngine::default()),
        &snapshot,
        rustx::model::ModelTimeoutPolicy::default(),
        clock.clone(),
    )
    .expect("valid context runtime");
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let mut execution = AgentExecution::new(
        AgentExecutionRequest {
            agent_id: AgentId::new("agent-205-boundary"),
            conversation_id: fixture.runtime.conversation_id().clone(),
            attempt_id: AttemptId::new(format!("attempt-205-{attempt}")),
            conversation: rustx::conversation::ConversationState::from_messages(vec![
                MessageBlock::User(UserMessageBlock {
                    id: MessageId::new(format!("msg-user-205-{attempt}")),
                    content: vec![UserContentBlock::Text(TextBlock {
                        text: "call the mcp tool".to_owned(),
                    })],
                    source: UserSource::Human,
                    kind: rustx::message::types::InboundKind::Message,
                    timestamp: None,
                }),
            ])
            .expect("valid fixture conversation"),
            initial_turn_trigger: rustx::runtime::inbound::InitialTurnTrigger::Continuation,
            model: support::attempt_model(model.clone(), "fake-model"),
        },
        capability,
        &cancellation,
        crate::agent::execution::AgentExecutionRuntimePolicy {
            model_timeout_policy: rustx::model::ModelTimeoutPolicy::default(),
            tool_deadline_policy: policy,
            monotonic_clock: clock.clone() as Arc<dyn MonotonicClock>,
            subagent_context: None,
            workflow_output: None,
        },
        context_runtime,
        &fixture.runtime,
        rustx::agent::AttemptLifecycle::inert(),
    )
    .expect("conversation identity matches the tool runtime");

    let (started_sender, started) = watch::channel(false);
    let (progress_sender, progress) = watch::channel(0);
    let observer = ExecutionSignals {
        started: started_sender,
        progress: progress_sender,
        ordered,
    };
    execution.observe(&observer);
    let (armed_signal, armed) =
        crate::agent::execution::test_sync::ToolDeadlineArmedSignal::install();
    execution.install_tool_deadline_armed_signal(armed_signal);
    let (physical_pause, physical_won, release_physical) =
        crate::agent::execution::test_sync::ToolPhysicalSettlementPause::install();
    execution.install_tool_physical_settlement_pause(physical_pause);

    let controls = Controls {
        started,
        progress,
        armed,
        physical_won,
        release_physical,
        clock,
        cancellation: cancellation.clone(),
    };
    let driver = tokio::spawn(controller(controls));
    let result = tokio::time::timeout(Duration::from_mins(2), execution.run())
        .await
        .expect("anti-hang guard: the manual-clock lifecycle always settles");
    driver.await.expect("boundary controller");
    common::durable_agent_result(result, fixture.store.as_ref())
}

/// Runs one attempt whose single model turn issues `count` calls of the same
/// MCP tool, in **one** parallel batch.
///
/// Sequencing matters here: the Agent Loop groups *adjacent* invocations
/// whose concurrency policy is `Parallel` into one concurrently executed
/// batch, so this is how a scenario puts many MCP requests in flight at the
/// same instant without any timing assumption.
async fn run_parallel_mcp_calls<C, F>(
    fixture: &common::NativeFixture,
    capability: rustx::capabilities::AttemptCapabilityLease,
    attempt: &str,
    tool_name: &str,
    count: usize,
    policy: ToolExecutionDeadlinePolicy,
    controller: C,
) -> common::DurableExecutionAudit
where
    C: FnOnce(Controls) -> F + Send + 'static,
    F: std::future::Future<Output = ()> + Send + 'static,
{
    run_mcp_turns(
        fixture,
        capability,
        attempt,
        &[vec![tool_name; count]],
        policy,
        None,
        controller,
    )
    .await
}

/// Runs one attempt with exactly one MCP tool call, recording the order in
/// which the runtime published that call's live progress reports and its
/// terminal fact.
async fn run_mcp_call_ordered<C, F>(
    fixture: &common::NativeFixture,
    capability: rustx::capabilities::AttemptCapabilityLease,
    attempt: &str,
    tool_name: &str,
    policy: ToolExecutionDeadlinePolicy,
    ordered: &Arc<OrderedObservations>,
    controller: C,
) -> common::DurableExecutionAudit
where
    C: FnOnce(Controls) -> F + Send + 'static,
    F: std::future::Future<Output = ()> + Send + 'static,
{
    run_mcp_turns(
        fixture,
        capability,
        attempt,
        &[vec![tool_name]],
        policy,
        Some(Arc::clone(ordered)),
        controller,
    )
    .await
}

/// Runs one attempt with exactly one MCP tool call.
async fn run_mcp_call<C, F>(
    fixture: &common::NativeFixture,
    capability: rustx::capabilities::AttemptCapabilityLease,
    attempt: &str,
    tool_name: &str,
    policy: ToolExecutionDeadlinePolicy,
    controller: C,
) -> common::DurableExecutionAudit
where
    C: FnOnce(Controls) -> F + Send + 'static,
    F: std::future::Future<Output = ()> + Send + 'static,
{
    run_mcp_calls(
        fixture,
        capability,
        attempt,
        &[tool_name],
        policy,
        controller,
    )
    .await
}

/// The canonical tool results of an audited attempt, in canonical order.
fn tool_results(
    audit: &common::DurableExecutionAudit,
) -> Vec<&rustx::tools::types::ToolExecutionResult> {
    audit
        .result
        .messages()
        .iter()
        .filter_map(|message| match message {
            MessageBlock::Tool(tool) => Some(&tool.result),
            _ => None,
        })
        .collect()
}

/// The single canonical tool result of an audited attempt.
fn single_tool_result(
    audit: &common::DurableExecutionAudit,
) -> &rustx::tools::types::ToolExecutionResult {
    let results = tool_results(audit);
    assert_eq!(
        results.len(),
        1,
        "an accepted ToolCall settles exactly once, canonically; outcome={:?}",
        audit.result.outcome
    );
    results[0]
}

/// Every per-call execution fact of the audited attempt, in durable order.
fn execution_facts(audit: &common::DurableExecutionAudit) -> Vec<&RuntimeEvent> {
    audit
        .event_history
        .iter()
        .filter(|event| {
            matches!(
                event,
                RuntimeEvent::ToolExecutionStarted { .. }
                    | RuntimeEvent::ToolExecutionDeadlineFired { .. }
                    | RuntimeEvent::ToolExecutionCancellationRequested { .. }
                    | RuntimeEvent::ToolExecutionSettlementObserved { .. }
                    | RuntimeEvent::ToolExecutionSettlementControlFailed { .. }
                    | RuntimeEvent::ToolExecutionCompleted { .. }
            )
        })
        .collect()
}

/// Executes one published MCP tool directly through the executor boundary,
/// without the Agent Loop.
///
/// Used where the scenario is about the executor's own pre-frontier
/// behaviour and an attempt capability lease would be the wrong instrument
/// (a lease pins the physical generation open, which is exactly what a drain
/// scenario must not do).
async fn direct_mcp_call(
    fixture: &common::NativeFixture,
    snapshot: &Arc<rustx::capabilities::CapabilitySnapshot>,
    tool_name: &str,
) -> rustx::tools::types::ToolExecutionResult {
    struct NoProgress;
    impl rustx::tools::executor::ProgressReporter for NoProgress {
        fn report(&self, _progress: rustx::tools::types::ToolProgress) {}
    }
    let definition = snapshot
        .tool_registry()
        .definitions()
        .into_iter()
        .find(|definition| definition.name == tool_name)
        .expect("the published catalog carries the tool");
    let executor = snapshot.tool_registry().executor(&definition.id);
    let progress = NoProgress;
    rustx::tools::executor::ToolExecutor::start(
        executor.as_ref(),
        rustx::tools::types::ToolInvocation {
            id: rustx::tools::types::ToolInvocationId::Agent {
                call_id: rustx::runtime::identity::ToolCallId::new("direct-call"),
            },
            tool_id: definition.id.clone(),
            tool_name: tool_name.to_owned(),
            mode: rustx::tools::types::ToolInvocationMode::Foreground,
            arguments: serde_json::json!({}),
        },
        rustx::tools::executor::ToolExecutionContext::new(
            fixture.runtime.conversation_id(),
            None,
            rustx::runtime::ExecutionCancellation::detached(
                rustx::runtime::CancellationSignal::new(),
                CancellationReason::UserRequested,
            ),
            fixture.runtime.workspace(),
            &progress,
            fixture.runtime.artifacts(),
            fixture.runtime.tool_output(),
            fixture.runtime.environment(),
        ),
    )
    .completion
    .await
}

/// Waits for one journal entry from inside an async controller.
///
/// Same rendezvous as [`wait_for_journal_entry`]: the entry is a fact the
/// server process wrote before acting, and the wall-clock bound is only an
/// anti-hang guard.
async fn await_journal_entry(control: &recovery::RecoveryControl, entry: &str) {
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        if control.journal_entries().iter().any(|line| line == entry) {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "anti-hang guard: the fixture never journaled {entry:?}; journal: {:?}",
            control.journal_entries()
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

/// The journal entry proving the server accepted a `tools/call` for `tool`.
fn accepted_call_entry(tool: &str) -> String {
    format!("{}{tool}", recovery::JOURNAL_CALL_PREFIX)
}

/// Waits for one journal entry, with a wall-clock anti-hang guard.
///
/// The entry itself is the ordering proof: the fixture writes it only after
/// the corresponding protocol event actually happened in the server process.
/// The guard only bounds how long the parent is willing to wait for a fact
/// that must eventually appear.
fn wait_for_journal_entry(control: &recovery::RecoveryControl, entry: &str) {
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    loop {
        if control.journal_entries().iter().any(|line| line == entry) {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "anti-hang guard: the fixture never journaled {entry:?}; journal: {:?}",
            control.journal_entries()
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

// ---------------------------------------------------------------------------
// Liveness: the generic hard deadline bounds an MCP call that never answers
// ---------------------------------------------------------------------------

/// Issue #205 regressions 1, 4 and 6: an MCP `tools/call` that the server
/// accepts and never answers is bounded by the **generic** Issue #204 hard
/// deadline, the deadline propagates `notifications/cancelled` to the
/// server, and — because no correlated remote response ever exists — the
/// one canonical result is `OutcomeUnknown`, never `TimedOut`.
///
/// Synchronization proof: the controller waits for the fixture's in-band
/// dispatch progress notification (the server provably received the
/// request), then crosses exactly the armed hard deadline the lifecycle
/// published. The server's own journal proves the cancellation notification
/// arrived.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unanswered_mcp_call_is_bounded_by_the_generic_hard_deadline() {
    if recovery::serve_if_recovery_fixture_mode().await {
        return;
    }
    let fixture = common::native_fixture_without_extensions();
    let control = recovery::RecoveryControl::new(fixture.dir().path());
    let capability = recovery_capability(
        &fixture.runtime,
        "boundary_suites::mcp_recovery::an_unanswered_mcp_call_is_bounded_by_the_generic_hard_deadline",
        &control,
        &recovery::RecoveryScript::default(),
    )
    .await;
    let lease = capability.coordinator.acquire_attempt_lease();
    let dispatch_gate = control.clone();
    let audit = run_mcp_call(
        &fixture,
        lease,
        "hang",
        recovery::TOOL_HANG,
        ToolExecutionDeadlinePolicy {
            hard_deadline: Duration::from_secs(5),
            idle_liveness: None,
        },
        move |mut controls| async move {
            controls.wait_started().await;
            // The dispatch gate: the server journaled the accepted request,
            // so it provably crossed the effect frontier.
            await_journal_entry(&dispatch_gate, &accepted_call_entry(recovery::TOOL_HANG)).await;
            let deadline = controls.cross_hard_deadline().await;
            assert_eq!(
                deadline, 5_000,
                "the frontier is the executor start, at clock zero"
            );
        },
    )
    .await;

    let result = single_tool_result(&audit);
    let ToolExecutionStatus::OutcomeUnknown { detail } = &result.status else {
        panic!(
            "a dispatched MCP call with no correlated remote response is unknown, \
             never a proven timeout: {:?}",
            result.status
        );
    };
    assert!(
        detail.contains("connection generation 1"),
        "the ambiguity names the transport generation it happened on: {detail}"
    );
    let facts = execution_facts(&audit);
    assert!(matches!(
        facts.first(),
        Some(RuntimeEvent::ToolExecutionStarted { .. })
    ));
    assert!(
        facts.iter().any(|fact| matches!(
            fact,
            RuntimeEvent::ToolExecutionDeadlineFired {
                kind: ToolDeadlineKind::Hard,
                ..
            }
        )),
        "the generic hard deadline is what bounded the call: {facts:?}"
    );
    assert!(
        facts.iter().any(|fact| matches!(
            fact,
            RuntimeEvent::ToolExecutionSettlementObserved {
                certainty: rustx::tools::deadline::ToolSettlementCertainty::Unconfirmed,
                ..
            }
        )),
        "the MCP executor's settlement authority returned unconfirmed evidence: {facts:?}"
    );
    assert!(
        !facts.iter().any(|fact| matches!(
            fact,
            RuntimeEvent::ToolExecutionSettlementControlFailed { .. }
        )),
        "the MCP settlement plane returns; the guard never fires: {facts:?}"
    );
    assert_eq!(
        facts
            .iter()
            .filter(|fact| matches!(fact, RuntimeEvent::ToolExecutionCompleted { .. }))
            .count(),
        1,
        "exactly one terminal fact, terminal-last"
    );
    assert!(matches!(
        facts.last(),
        Some(RuntimeEvent::ToolExecutionCompleted { .. })
    ));

    // The deadline propagated the strongest cancellation the negotiated
    // protocol defines: the server observed `notifications/cancelled`.
    wait_for_journal_entry(
        &control,
        &format!("{}1", recovery::JOURNAL_CANCELLED_PREFIX),
    );
    assert_eq!(
        control.accepted_calls(recovery::TOOL_HANG),
        1,
        "the ambiguous invocation reached the server exactly once"
    );
    drop(capability);
}

/// Issue #205 regressions 2 and 3: genuine remote MCP progress refreshes the
/// generic idle-liveness watchdog and can never extend the generic hard
/// deadline.
///
/// Synchronization proof: every clock advance is 900ms against a 1000ms idle
/// window, and each advance happens only after the previous remote progress
/// notification was **observed**. The progress fanout refreshes the idle
/// watchdog before it calls any observer, so observing a progress report is
/// a happens-after of its own idle refresh — the advance can therefore never
/// race an unpublished refresh. Three refreshed windows later the total
/// elapsed time crosses the immutable 3000ms hard deadline while the current
/// idle window (2700..3700) is still open, so the *hard* deadline is the
/// winner: progress kept idle alive and moved the hard bound not at all.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mcp_progress_refreshes_idle_liveness_and_never_extends_the_hard_deadline() {
    if recovery::serve_if_recovery_fixture_mode().await {
        return;
    }
    let fixture = common::native_fixture_without_extensions();
    let control = recovery::RecoveryControl::new(fixture.dir().path());
    let capability = recovery_capability(
        &fixture.runtime,
        "boundary_suites::mcp_recovery::mcp_progress_refreshes_idle_liveness_and_never_extends_the_hard_deadline",
        &control,
        &recovery::RecoveryScript {
            hang_pulses: 3,
            ..recovery::RecoveryScript::default()
        },
    )
    .await;
    let lease = capability.coordinator.acquire_attempt_lease();
    let control_handle = control.clone();
    let dispatch_gate = control.clone();
    let audit = run_mcp_call(
        &fixture,
        lease,
        "hang",
        recovery::TOOL_HANG,
        ToolExecutionDeadlinePolicy {
            hard_deadline: Duration::from_secs(3),
            idle_liveness: Some(Duration::from_secs(1)),
        },
        move |mut controls| async move {
            controls.wait_started().await;
            await_journal_entry(&dispatch_gate, &accepted_call_entry(recovery::TOOL_HANG)).await;
            for pulse in 1..=3_u32 {
                // 900 < 1000: the current idle window survives this advance.
                controls.clock.advance(900);
                control_handle.release_hang_pulse(pulse);
                // The refreshed window is published before this returns.
                controls.wait_progress_at_least(pulse).await;
            }
            // t = 2700, newest idle window 2700..3700, hard deadline 3000.
            controls.clock.advance(900);
        },
    )
    .await;

    let facts = execution_facts(&audit);
    let fired: Vec<ToolDeadlineKind> = facts
        .iter()
        .filter_map(|fact| match fact {
            RuntimeEvent::ToolExecutionDeadlineFired { kind, .. } => Some(*kind),
            _ => None,
        })
        .collect();
    assert_eq!(
        fired,
        vec![ToolDeadlineKind::Hard],
        "continuous remote progress kept the idle watchdog alive, and the immutable \
         hard deadline still bounded the call"
    );
    let progress_facts = audit
        .event_history
        .iter()
        .filter(|event| matches!(event, RuntimeEvent::ToolExecutionProgress { .. }))
        .count();
    assert_eq!(
        progress_facts, 3,
        "exactly the three released remote pulses were forwarded as durable \
         liveness evidence; the executor fabricates none"
    );
    assert!(matches!(
        single_tool_result(&audit).status,
        ToolExecutionStatus::OutcomeUnknown { .. }
    ));
    drop(capability);
}

// ---------------------------------------------------------------------------
// Transport loss, reconnection, and the no-replay contract
// ---------------------------------------------------------------------------

/// Issue #205 regressions 6, 8, 9, 10 and 17: the complete
/// loss-then-recovery sequence over one published capability generation.
///
/// ```text
/// generation 1  -- call echo --> received, then the server dies
///               <- OutcomeUnknown (post-frontier, no correlated response)
/// generation 2  -- call echo --> received, answered
///               <- Success
/// ```
///
/// Synchronization proof: generation 1 is configured to exit *after*
/// journaling the accepted call and emitting its dispatch progress
/// notification, so the request provably crossed the effect frontier before
/// the transport died — the parent does not have to guess. The journal then
/// proves the whole contract by counting: `echo` was accepted exactly twice
/// across two server processes, i.e. exactly once per call the model issued.
/// A replay of the ambiguous invocation would show three.
///
/// Both calls run inside one attempt, so the second is served by a
/// replacement transport under the same conversation identity, the same
/// published capability generation, and the same durable authority.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn transport_loss_after_dispatch_is_unknown_and_reconnect_never_replays_it() {
    if recovery::serve_if_recovery_fixture_mode().await {
        return;
    }
    let fixture = common::native_fixture_without_extensions();
    let control = recovery::RecoveryControl::new(fixture.dir().path());
    // Only generation 1 dies with the request in flight; generation 2 serves
    // normally.
    let capability = recovery_capability(
        &fixture.runtime,
        "boundary_suites::mcp_recovery::transport_loss_after_dispatch_is_unknown_and_reconnect_never_replays_it",
        &control,
        &recovery::RecoveryScript {
            die_generations: vec![1],
            ..recovery::RecoveryScript::default()
        },
    )
    .await;
    let published = capability.definition_names();
    assert!(
        published.contains(&recovery::TOOL_ECHO.to_owned()),
        "the published capability generation carries the fixture catalog: {published:?}"
    );

    let audit = run_mcp_calls(
        &fixture,
        capability.coordinator.acquire_attempt_lease(),
        "loss",
        &[recovery::TOOL_ECHO, recovery::TOOL_ECHO],
        ToolExecutionDeadlinePolicy {
            hard_deadline: Duration::from_mins(1),
            idle_liveness: None,
        },
        |_controls| async move {},
    )
    .await;

    let results = tool_results(&audit);
    assert_eq!(
        results.len(),
        2,
        "both accepted ToolCalls settle exactly once each; outcome={:?}",
        audit.result.outcome
    );
    let ToolExecutionStatus::OutcomeUnknown { detail } = &results[0].status else {
        panic!(
            "a transport that dies with the request in flight leaves the external \
             outcome unknown: {:?}",
            results[0].status
        );
    };
    assert!(
        detail.contains("connection generation 1"),
        "the ambiguity is attributed to the transport generation that died: {detail}"
    );
    assert!(
        matches!(results[1].status, ToolExecutionStatus::Success),
        "a later ToolCall is served by the replacement generation: {:?}",
        results[1].status
    );
    let rendered = results[1].model_facing_projection().as_text();
    assert!(
        rendered.contains("generation 2"),
        "the replacement transport actually served the later call: {rendered}"
    );

    // The published capability generation is untouched by transport loss:
    // last-known-good capability knowledge is not transport availability.
    assert_eq!(
        capability.definition_names(),
        published,
        "a dead transport never erases validated capability knowledge"
    );
    wait_for_journal_entry(&control, recovery::JOURNAL_DIED);
    assert_eq!(
        control.established_generations(),
        2,
        "exactly one bounded replacement transport was established"
    );
    assert_eq!(
        control.accepted_calls(recovery::TOOL_ECHO),
        2,
        "exactly one accepted request per model-issued call: the ambiguous \
         invocation was never resubmitted to the replacement generation"
    );
    assert_eq!(
        execution_facts(&audit)
            .iter()
            .filter(|fact| matches!(fact, RuntimeEvent::ToolExecutionCompleted { .. }))
            .count(),
        2,
        "repeated loss and reconnection signals never duplicate canonical settlement"
    );
    drop(capability);
}

/// Issue #205 regressions 7 and 9, and the boundedness of reconnection: an
/// ambiguous call is never replayed onto a replacement generation, and the
/// dispatch whose *own* transport could not be established fails as an
/// ordinary `Failed` rather than claiming unknown external side effects.
///
/// ```text
/// generation 1 -- call echo --> received, then the server dies
///              <- OutcomeUnknown        (post-frontier)
/// generation 2 -- refuses the handshake
///              <- Failed                (pre-frontier: no request existed)
/// ```
///
/// Synchronization proof: generation 1 journals the accepted call before it
/// exits, and generation 2 journals its refusal before it exits, so both
/// phases are facts written by the server processes rather than parent-side
/// guesses. The `echo` acceptance count stays at exactly one across the
/// whole sequence: the failed reconnection had no in-flight request to
/// resubmit, and nothing in the connection owner can hand a previously
/// dispatched request to a new generation.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_failed_reconnect_is_a_pre_frontier_failure_and_never_replays_the_ambiguous_call() {
    if recovery::serve_if_recovery_fixture_mode().await {
        return;
    }
    let fixture = common::native_fixture_without_extensions();
    let control = recovery::RecoveryControl::new(fixture.dir().path());
    let capability = recovery_capability(
        &fixture.runtime,
        "boundary_suites::mcp_recovery::a_failed_reconnect_is_a_pre_frontier_failure_and_never_replays_the_ambiguous_call",
        &control,
        &recovery::RecoveryScript {
            die_generations: vec![1],
            refuse_generations: vec![2],
            ..recovery::RecoveryScript::default()
        },
    )
    .await;

    let audit = run_mcp_calls(
        &fixture,
        capability.coordinator.acquire_attempt_lease(),
        "reconnect",
        &[recovery::TOOL_ECHO, recovery::TOOL_ECHO],
        ToolExecutionDeadlinePolicy {
            hard_deadline: Duration::from_mins(1),
            idle_liveness: None,
        },
        |_controls| async move {},
    )
    .await;

    let results = tool_results(&audit);
    assert_eq!(
        results.len(),
        2,
        "both accepted ToolCalls settle exactly once each; outcome={:?}",
        audit.result.outcome
    );
    assert!(
        matches!(
            results[0].status,
            ToolExecutionStatus::OutcomeUnknown { .. }
        ),
        "the request crossed the frontier and no correlated response exists: {:?}",
        results[0].status
    );
    let ToolExecutionStatus::Failed { error } = &results[1].status else {
        panic!(
            "a dispatch whose transport could never be established is an ordinary \
             failure, never unknown: {:?}",
            results[1].status
        );
    };
    assert!(
        error.contains("never reached the server"),
        "the diagnostic states that no request existed: {error}"
    );
    assert!(
        error.contains("connection history"),
        "the bounded typed connection facts reach the diagnostic: {error}"
    );

    wait_for_journal_entry(&control, recovery::JOURNAL_DIED);
    wait_for_journal_entry(&control, &format!("{}2", recovery::JOURNAL_REFUSED_PREFIX));
    assert_eq!(
        control.established_generations(),
        2,
        "exactly one bounded replacement attempt was made, not a reconnect loop"
    );
    assert_eq!(
        control.accepted_calls(recovery::TOOL_ECHO),
        1,
        "the ambiguous invocation was never resubmitted anywhere"
    );
    drop(capability);
}

/// Issue #205 regression 14, direction A: **cancellation wins**
/// deterministically when no correlated remote response can exist yet.
///
/// Synchronization proof: the `hang` tool never answers a dispatched call,
/// and the controller waits for the fixture's in-band dispatch notification
/// (the request provably reached the server) before it cancels. There is
/// therefore no correlated remote response at any point, the executor
/// propagates
/// `notifications/cancelled` — which the server journals — and the canonical
/// result is exactly one `OutcomeUnknown`, never a fabricated `Cancelled`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancellation_without_a_remote_response_is_exactly_one_outcome_unknown() {
    if recovery::serve_if_recovery_fixture_mode().await {
        return;
    }
    let fixture = common::native_fixture_without_extensions();
    let control = recovery::RecoveryControl::new(fixture.dir().path());
    let capability = recovery_capability(
        &fixture.runtime,
        "boundary_suites::mcp_recovery::cancellation_without_a_remote_response_is_exactly_one_outcome_unknown",
        &control,
        &recovery::RecoveryScript::default(),
    )
    .await;
    let lease = capability.coordinator.acquire_attempt_lease();
    let dispatch_gate = control.clone();
    let audit = run_mcp_call(
        &fixture,
        lease,
        "cancel",
        recovery::TOOL_HANG,
        ToolExecutionDeadlinePolicy {
            hard_deadline: Duration::from_mins(1),
            idle_liveness: None,
        },
        move |mut controls| async move {
            controls.wait_started().await;
            await_journal_entry(&dispatch_gate, &accepted_call_entry(recovery::TOOL_HANG)).await;
            // The `hang` tool never answers, so no correlated remote
            // response can exist when cancellation is delivered.
            controls.cancellation.cancel();
        },
    )
    .await;

    let result = single_tool_result(&audit);
    let ToolExecutionStatus::OutcomeUnknown { detail } = &result.status else {
        panic!(
            "an unconfirmed post-dispatch cancellation is unknown, never a proven \
             cancellation: {:?}",
            result.status
        );
    };
    assert!(
        detail.contains("remote termination could not be confirmed"),
        "the diagnostic states exactly what could not be proven: {detail}"
    );
    let facts = execution_facts(&audit);
    // The lifecycle has two admissible linearizations here, and this test
    // deliberately constrains neither: the attempt's cancellation may win
    // the loop's arbitration, or the executor's own operation may observe
    // the same cancellation first and return its honest unconfirmed result
    // through the physical completion plane. What must hold in *both* is
    // that no settlement evidence claims confirmation and that no settlement
    // control-plane failure occurs — the MCP settlement authority always
    // returns.
    assert!(
        !facts.iter().any(|fact| matches!(
            fact,
            RuntimeEvent::ToolExecutionSettlementObserved {
                certainty: rustx::tools::deadline::ToolSettlementCertainty::Confirmed,
                ..
            } | RuntimeEvent::ToolExecutionSettlementControlFailed { .. }
        )),
        "an unconfirmed post-dispatch cancellation never yields confirmed evidence          and never trips the settlement guard: {facts:?}"
    );
    assert_eq!(
        facts
            .iter()
            .filter(|fact| matches!(fact, RuntimeEvent::ToolExecutionCompleted { .. }))
            .count(),
        1,
        "exactly one canonical terminal settlement"
    );
    // The strongest cancellation the negotiated protocol defines was
    // actually propagated to the server.
    wait_for_journal_entry(
        &control,
        &format!("{}1", recovery::JOURNAL_CANCELLED_PREFIX),
    );
    assert_eq!(
        control.accepted_calls(recovery::TOOL_HANG),
        1,
        "cancellation never causes a second dispatch"
    );
    drop(capability);
}

/// Issue #205 regressions 14 and 15, direction B: a **correlated remote
/// response** that won the lifecycle's arbitration stays the terminal
/// winner, and a cancellation issued afterwards cannot reopen it.
///
/// Synchronization proof: the generic lifecycle's physical-settlement pause
/// parks the execution *after* the physical-completion branch won its biased
/// `select!` and *before* result normalization. The controller waits for that
/// park — which can only be reached once the MCP executor already produced
/// the remote result — then cancels the attempt, then releases the park. The
/// cancellation is therefore provably post-arbitration, not a timing guess.
/// The canonical result is the proven remote `Success`, no deadline or
/// cancellation fact is journaled for the call, and the server accepted the
/// request exactly once.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_remote_response_that_won_arbitration_survives_a_later_cancellation() {
    if recovery::serve_if_recovery_fixture_mode().await {
        return;
    }
    let fixture = common::native_fixture_without_extensions();
    let control = recovery::RecoveryControl::new(fixture.dir().path());
    let capability = recovery_capability(
        &fixture.runtime,
        "boundary_suites::mcp_recovery::a_remote_response_that_won_arbitration_survives_a_later_cancellation",
        &control,
        &recovery::RecoveryScript::default(),
    )
    .await;
    let lease = capability.coordinator.acquire_attempt_lease();
    let audit = run_mcp_call(
        &fixture,
        lease,
        "won",
        recovery::TOOL_ECHO,
        ToolExecutionDeadlinePolicy {
            hard_deadline: Duration::from_mins(1),
            idle_liveness: None,
        },
        move |mut controls| async move {
            controls.wait_physical_won().await;
            controls.cancellation.cancel();
            controls
                .release_physical
                .send(())
                .expect("the physical settlement pause stays installed");
        },
    )
    .await;

    let result = single_tool_result(&audit);
    assert!(
        matches!(result.status, ToolExecutionStatus::Success),
        "the proven remote result won arbitration and survives a later \
         cancellation: {:?}",
        result.status
    );
    let facts = execution_facts(&audit);
    assert!(
        !facts.iter().any(|fact| matches!(
            fact,
            RuntimeEvent::ToolExecutionDeadlineFired { .. }
                | RuntimeEvent::ToolExecutionCancellationRequested { .. }
                | RuntimeEvent::ToolExecutionSettlementObserved { .. }
        )),
        "a physical-completion winner emits no cancellation or settlement facts: {facts:?}"
    );
    assert_eq!(
        facts
            .iter()
            .filter(|fact| matches!(fact, RuntimeEvent::ToolExecutionCompleted { .. }))
            .count(),
        1,
        "exactly one canonical terminal settlement"
    );
    assert_eq!(control.accepted_calls(recovery::TOOL_ECHO), 1);
    drop(capability);
}

/// Issue #205 regression 18: after capability drain the MCP plane can no
/// longer publish anything.
///
/// The drain closes every transport generation the connection ever
/// established — the one that died in flight and the replacement that served
/// the later call — and proves each one settled. After it, nothing in the
/// MCP plane can act: an executor resolved from the pre-drain published
/// snapshot settles as an ordinary pre-frontier failure, no reconnection
/// spawns a server, and the fixture journal gains no further accepted
/// request. There is no detached reconnect, refresh, or notification task
/// that could publish a second terminal outcome or a stale capability
/// generation, because this issue introduces none: reconnection is owned by
/// the dispatching execution future and closes with the connection.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn drain_closes_every_connection_generation_and_refuses_reconnection() {
    if recovery::serve_if_recovery_fixture_mode().await {
        return;
    }
    let fixture = common::native_fixture_without_extensions();
    let control = recovery::RecoveryControl::new(fixture.dir().path());
    let capability = recovery_capability(
        &fixture.runtime,
        "boundary_suites::mcp_recovery::drain_closes_every_connection_generation_and_refuses_reconnection",
        &control,
        &recovery::RecoveryScript {
            die_generations: vec![1],
            ..recovery::RecoveryScript::default()
        },
    )
    .await;
    // Generation 1 dies in flight and generation 2 is established by the
    // second call, so drain has two transport generations to account for.
    let audit = run_mcp_calls(
        &fixture,
        capability.coordinator.acquire_attempt_lease(),
        "drain",
        &[recovery::TOOL_ECHO, recovery::TOOL_ECHO],
        ToolExecutionDeadlinePolicy {
            hard_deadline: Duration::from_mins(1),
            idle_liveness: None,
        },
        |_controls| async move {},
    )
    .await;
    let results = tool_results(&audit);
    assert_eq!(results.len(), 2);
    assert!(matches!(results[1].status, ToolExecutionStatus::Success));
    assert_eq!(control.established_generations(), 2);

    // The published capability generation is captured before drain, exactly
    // as a still-running consumer would hold it.
    let published = capability.coordinator.current_snapshot();
    capability
        .coordinator
        .drain_conversation_owned()
        .await
        .expect("every established transport generation proves settlement");
    let generations_after_drain = control.established_generations();

    let result = direct_mcp_call(&fixture, &published, recovery::TOOL_ECHO).await;
    assert!(
        matches!(result.status, ToolExecutionStatus::Failed { .. }),
        "a drained MCP plane settles later calls as ordinary pre-frontier failures, \
         never as a second terminal outcome: {:?}",
        result.status
    );
    assert_eq!(
        control.established_generations(),
        generations_after_drain,
        "drain closes publication authority: no reconnection may spawn a server after it"
    );
    assert_eq!(
        control.accepted_calls(recovery::TOOL_ECHO),
        2,
        "the drained plane accepted no further remote request"
    );
    drop(capability);
}

// ---------------------------------------------------------------------------
// Last-known-good capability publication
// ---------------------------------------------------------------------------

/// Issue #205 regression 11: a capability refresh that cannot produce a
/// complete validated generation keeps the last-known-good generation
/// authoritative, and never publishes an empty or partial catalog.
///
/// ```text
/// G1 authoritative  [echo, hang]
///   -> refresh: generation 2 refuses the handshake
///        -> candidate carries G1 forward verbatim
///        -> commit is a no-op: G1 stays authoritative, revision unchanged
///   -> the MCP source is reported unavailable (transport), while the
///      catalog (knowledge) is untouched
///   -> the retained generation's live transport keeps serving calls
/// ```
///
/// Synchronization proof: the refusing generation journals its refusal
/// before it exits, so the failed refresh is a fact the server process
/// wrote. The assertions then compare the published catalog and the
/// capability revision across the failed refresh, and a subsequent real call
/// proves the retained generation's connection owner still reconnects.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_failed_capability_refresh_keeps_the_last_known_good_generation() {
    if recovery::serve_if_recovery_fixture_mode().await {
        return;
    }
    let fixture = common::native_fixture_without_extensions();
    let control = recovery::RecoveryControl::new(fixture.dir().path());
    // Generation 2 — the one a refresh would establish — refuses to
    // handshake, so the refresh cannot produce a validated generation.
    let capability = recovery_capability(
        &fixture.runtime,
        "boundary_suites::mcp_recovery::a_failed_capability_refresh_keeps_the_last_known_good_generation",
        &control,
        &recovery::RecoveryScript {
            refuse_generations: vec![2],
            ..recovery::RecoveryScript::default()
        },
    )
    .await;
    let published = capability.definition_names();
    assert_eq!(
        published,
        vec![
            recovery::TOOL_ANNOUNCE.to_owned(),
            recovery::TOOL_ECHO.to_owned(),
            recovery::TOOL_HANG.to_owned(),
        ],
        "G1 is the validated last-known-good catalog"
    );
    let revision_before = capability.coordinator.current_snapshot().revision();

    let candidate = capability
        .coordinator
        .prepare_candidate()
        .await
        .expect("an unreachable optional MCP source never fails preparation");
    assert!(
        matches!(
            candidate
                .availability()
                .get(&rustx::capabilities::CapabilitySourceId::Mcp(
                    capability.server_id.clone()
                )),
            Some(rustx::capabilities::CapabilitySourceState::Unavailable { .. })
        ),
        "the refresh reports the transport as currently unavailable"
    );
    capability
        .coordinator
        .commit(candidate)
        .expect("the carried-forward candidate commits");

    wait_for_journal_entry(&control, &format!("{}2", recovery::JOURNAL_REFUSED_PREFIX));
    assert_eq!(
        capability.definition_names(),
        published,
        "a failed refresh never replaces the authoritative catalog with an empty \
         or partial one"
    );
    assert_eq!(
        capability.coordinator.current_snapshot().revision(),
        revision_before,
        "a failed refresh fabricates no capability revision"
    );

    // Transport availability and capability knowledge are different facts,
    // and a failed refresh disturbs neither: the retained generation's live
    // transport keeps serving calls.
    let audit = run_mcp_call(
        &fixture,
        capability.coordinator.acquire_attempt_lease(),
        "after-failed-refresh",
        recovery::TOOL_ECHO,
        ToolExecutionDeadlinePolicy {
            hard_deadline: Duration::from_mins(1),
            idle_liveness: None,
        },
        |_controls| async move {},
    )
    .await;
    let result = single_tool_result(&audit);
    assert!(
        matches!(result.status, ToolExecutionStatus::Success),
        "the carried-forward generation is still usable once a transport exists: {:?}",
        result.status
    );
    assert!(
        result
            .model_facing_projection()
            .as_text()
            .contains("generation 1"),
        "the retained last-known-good generation's own live transport served the          call: a failed refresh never retires it"
    );
    drop(capability);
}

/// Issue #205 regression 12: a complete validated refresh replaces the
/// previous capability generation atomically.
///
/// Generation 2 publishes a catalog with one additional tool. Before the
/// commit the authoritative snapshot carries exactly the old catalog; after
/// it, exactly the new one. Snapshots are immutable values, so no observer
/// can see a mixture: the swap is one store under the capability state lock,
/// and the previous physical generation retires at that same point.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_successful_refresh_atomically_replaces_the_previous_generation() {
    if recovery::serve_if_recovery_fixture_mode().await {
        return;
    }
    let fixture = common::native_fixture_without_extensions();
    let control = recovery::RecoveryControl::new(fixture.dir().path());
    let capability = recovery_capability(
        &fixture.runtime,
        "boundary_suites::mcp_recovery::a_successful_refresh_atomically_replaces_the_previous_generation",
        &control,
        &recovery::RecoveryScript {
            extra_tool_generations: vec![2],
            ..recovery::RecoveryScript::default()
        },
    )
    .await;
    let before = capability.coordinator.current_snapshot();
    assert_eq!(
        capability.definition_names(),
        vec![
            recovery::TOOL_ANNOUNCE.to_owned(),
            recovery::TOOL_ECHO.to_owned(),
            recovery::TOOL_HANG.to_owned(),
        ]
    );

    let candidate = capability
        .coordinator
        .prepare_candidate()
        .await
        .expect("generation 2 publishes a complete catalog");
    // The candidate exists and is complete, and the authoritative generation
    // is still exactly G1: preparation never mutates published knowledge.
    assert_eq!(
        capability.definition_names(),
        vec![
            recovery::TOOL_ANNOUNCE.to_owned(),
            recovery::TOOL_ECHO.to_owned(),
            recovery::TOOL_HANG.to_owned(),
        ],
        "a prepared candidate is not authoritative until it commits"
    );
    capability
        .coordinator
        .commit(candidate)
        .expect("the complete validated candidate commits");

    assert_eq!(
        capability.definition_names(),
        vec![
            recovery::TOOL_ANNOUNCE.to_owned(),
            recovery::TOOL_ECHO.to_owned(),
            recovery::TOOL_EXTRA.to_owned(),
            recovery::TOOL_HANG.to_owned(),
        ],
        "the new generation is published whole"
    );
    assert!(
        capability.coordinator.current_snapshot().revision() > before.revision(),
        "a complete refresh publishes a new capability revision"
    );
    // The pre-refresh snapshot value is unchanged: an already-admitted
    // attempt keeps executing against exactly the generation it pinned.
    assert_eq!(
        before
            .tool_registry()
            .definitions()
            .into_iter()
            .map(|definition| definition.name)
            .collect::<Vec<_>>(),
        vec![
            recovery::TOOL_ANNOUNCE.to_owned(),
            recovery::TOOL_ECHO.to_owned(),
            recovery::TOOL_HANG.to_owned(),
        ],
        "the previously pinned snapshot is an immutable value"
    );
    drop(capability);
}

/// Issue #205 regression 13: candidate capability state under construction
/// is never observable to Tool admission.
///
/// Synchronization proof: the coordinator's connect-ownership pause parks the
/// refresh **inside** candidate construction, at the exact instant the new
/// server process exists and its handshake has not started — the widest
/// window in which a partial candidate could possibly leak. A second
/// participant then acquires an attempt capability lease and resolves tools
/// while the candidate is provably parked there. It observes exactly the
/// complete authoritative generation: never a partial catalog, never an
/// empty one.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tool_admission_never_observes_a_candidate_under_construction() {
    if recovery::serve_if_recovery_fixture_mode().await {
        return;
    }
    let fixture = common::native_fixture_without_extensions();
    let control = recovery::RecoveryControl::new(fixture.dir().path());
    let capability = recovery_capability(
        &fixture.runtime,
        "boundary_suites::mcp_recovery::tool_admission_never_observes_a_candidate_under_construction",
        &control,
        &recovery::RecoveryScript {
            extra_tool_generations: vec![2],
            ..recovery::RecoveryScript::default()
        },
    )
    .await;
    let authoritative = vec![
        recovery::TOOL_ANNOUNCE.to_owned(),
        recovery::TOOL_ECHO.to_owned(),
        recovery::TOOL_HANG.to_owned(),
    ];
    assert_eq!(capability.definition_names(), authoritative);

    let pause = Arc::new(rustx::tools::mcp::test_sync::ConnectOwnershipPause::default());
    capability
        .coordinator
        .install_connect_ownership_pause(pause.clone());
    let refreshing = capability.coordinator.clone();
    let refresh = tokio::spawn(async move { refreshing.prepare_candidate().await });
    // The candidate is now provably parked mid-construction.
    pause.wait_entered().await;

    let lease = capability.coordinator.acquire_attempt_lease();
    let observed: Vec<String> = lease
        .snapshot()
        .tool_registry()
        .definitions()
        .into_iter()
        .map(|definition| definition.name)
        .collect();
    assert_eq!(
        observed, authoritative,
        "an admitted attempt resolves against the complete authoritative \
         generation while a candidate is under construction"
    );
    drop(lease);

    pause.release();
    let candidate = refresh
        .await
        .expect("refresh task")
        .expect("the parked candidate completes");
    capability
        .coordinator
        .commit(candidate)
        .expect("the complete candidate commits");
    assert_eq!(
        capability.definition_names(),
        vec![
            recovery::TOOL_ANNOUNCE.to_owned(),
            recovery::TOOL_ECHO.to_owned(),
            recovery::TOOL_EXTRA.to_owned(),
            recovery::TOOL_HANG.to_owned(),
        ],
        "only the completed candidate becomes authoritative"
    );
    drop(capability);
}

/// Issue #205 regressions 16 and 17: confirmed protocol corruption fails
/// closed, and the poisoned connection generation is replaced for later work
/// without replaying the invocation it poisoned.
///
/// ```text
/// generation 1  -- call echo --> received, answered with a structurally
///                                invalid MCP/JSON-RPC line
///               <- OutcomeUnknown, generation poisoned (never a guessed
///                  or synthesized success)
/// generation 2  -- call echo --> received, answered
///               <- Success
/// ```
///
/// Synchronization proof: the fixture journals both the accepted call and
/// the corrupt line it emitted before the client can observe anything, so
/// "corruption happened after this dispatch" is a server-written fact. The
/// `echo` acceptance count across both server processes is exactly two — one
/// per model-issued call — proving the poisoned invocation was never
/// resubmitted onto the replacement generation.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_poisoned_generation_fails_closed_and_is_replaced_without_replay() {
    if recovery::serve_if_recovery_fixture_mode().await {
        return;
    }
    let fixture = common::native_fixture_without_extensions();
    let control = recovery::RecoveryControl::new(fixture.dir().path());
    let capability = recovery_capability(
        &fixture.runtime,
        "boundary_suites::mcp_recovery::a_poisoned_generation_fails_closed_and_is_replaced_without_replay",
        &control,
        &recovery::RecoveryScript {
            corrupt_generations: vec![1],
            ..recovery::RecoveryScript::default()
        },
    )
    .await;

    let audit = run_mcp_calls(
        &fixture,
        capability.coordinator.acquire_attempt_lease(),
        "poison",
        &[recovery::TOOL_ECHO, recovery::TOOL_ECHO],
        ToolExecutionDeadlinePolicy {
            hard_deadline: Duration::from_mins(1),
            idle_liveness: None,
        },
        |_controls| async move {},
    )
    .await;

    let results = tool_results(&audit);
    assert_eq!(
        results.len(),
        2,
        "both accepted ToolCalls settle exactly once each; outcome={:?}",
        audit.result.outcome
    );
    let ToolExecutionStatus::OutcomeUnknown { detail } = &results[0].status else {
        panic!(
            "corruption after dispatch fails closed as an unknown outcome, never a \
             synthesized success and never a guessed failure: {:?}",
            results[0].status
        );
    };
    assert!(
        detail.contains("protocol violation"),
        "the diagnostic names the confirmed structural violation: {detail}"
    );
    assert!(
        results[0].content.is_empty(),
        "a poisoned generation never synthesizes result content"
    );
    assert!(
        matches!(results[1].status, ToolExecutionStatus::Success),
        "a healthy replacement generation serves later work: {:?}",
        results[1].status
    );

    wait_for_journal_entry(&control, recovery::JOURNAL_CORRUPTED);
    assert_eq!(
        control.established_generations(),
        2,
        "the poisoned generation is retired and replaced exactly once"
    );
    assert_eq!(
        control.accepted_calls(recovery::TOOL_ECHO),
        2,
        "the poisoned invocation was never resubmitted to the replacement"
    );
    drop(capability);
}

/// Issue #205: remote liveness evidence that arrives immediately before the
/// correlated response is never discarded, in either direction of the two
/// races an MCP client cannot avoid.
///
/// The `announce` tool emits one progress notification and then answers, with
/// no gate between them, so on the one ordered transport the notification
/// always precedes the response. Two adapter-owned windows could lose it, and
/// this exercises both:
///
/// - **the subscription window.** rmcp mints a request's progress token
///   inside `send_cancellable_request`, so the dispatching call can only
///   subscribe after the request is enqueued. A notification that arrives
///   first is held by the adapter's progress router and handed to the
///   subscription when it is created.
/// - **the biased arbitration.** A correlated response outranks progress in
///   the executor's `select!`, so a response that is already ready would
///   otherwise end the call with the notification still queued. The executor
///   drains the subscription before classifying the response.
///
/// # Where the adapter's ownership ends
///
/// Delivery of an inbound notification *into* the adapter is rmcp's, not
/// rustX's: rmcp's service loop hands a notification to the handler on a
/// spawned task, while it resolves a response's local responder inline. On
/// an ordered transport the notification is therefore always *received*
/// first, but the handler task that routes it may not have run when the
/// response resolves. rustX cannot discard evidence it has never been given,
/// so this suite asserts what the adapter actually owns.
///
/// That boundary is also why it is not a liveness hazard: this window can
/// only lose a notification when the correlated response has *already*
/// arrived, so the call settles immediately and no idle watchdog is ever
/// consulted. The false-idle case — where evidence must survive because the
/// call keeps running — is proven deterministically by the gated fixtures in
/// `mcp_progress_refreshes_idle_liveness_and_never_extends_the_hard_deadline`
/// and `concurrent_mcp_progress_refreshes_every_idle_watchdog_above_the_old_router_bound`,
/// and at the capacity boundary by `tools::mcp::progress_router_tests`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn remote_progress_delivered_just_before_the_response_is_never_discarded() {
    if recovery::serve_if_recovery_fixture_mode().await {
        return;
    }
    let fixture = common::native_fixture_without_extensions();
    let control = recovery::RecoveryControl::new(fixture.dir().path());
    let capability = recovery_capability(
        &fixture.runtime,
        "boundary_suites::mcp_recovery::remote_progress_delivered_just_before_the_response_is_never_discarded",
        &control,
        &recovery::RecoveryScript::default(),
    )
    .await;
    let audit = run_mcp_call(
        &fixture,
        capability.coordinator.acquire_attempt_lease(),
        "announce",
        recovery::TOOL_ANNOUNCE,
        ToolExecutionDeadlinePolicy {
            hard_deadline: Duration::from_mins(1),
            idle_liveness: None,
        },
        |_controls| async move {},
    )
    .await;

    let result = single_tool_result(&audit);
    assert!(
        matches!(result.status, ToolExecutionStatus::Success),
        "the correlated remote response is the terminal outcome: {:?}",
        result.status
    );
    let progress_facts: Vec<&RuntimeEvent> = audit
        .event_history
        .iter()
        .filter(|event| matches!(event, RuntimeEvent::ToolExecutionProgress { .. }))
        .collect();
    // At most one, never more: the executor forwards what the server sent
    // and fabricates nothing, and the router never duplicates a notification
    // between its pre-subscription window and the subscription.
    assert!(
        progress_facts.len() <= 1,
        "the executor forwards the one notification the server sent and invents \
         none: {progress_facts:?}"
    );
    if let Some(RuntimeEvent::ToolExecutionProgress { progress, .. }) = progress_facts.first() {
        // Whatever was forwarded is the peer's own value, unmodified.
        assert_eq!(progress.completed, Some(1.0));
        assert_eq!(progress.total, Some(4.0));
    }
    // Terminal-last: the liveness fact precedes the call's terminal fact.
    let facts = execution_facts(&audit);
    assert!(matches!(
        facts.last(),
        Some(RuntimeEvent::ToolExecutionCompleted { .. })
    ));
    assert_eq!(control.accepted_calls(recovery::TOOL_ANNOUNCE), 1);
    drop(capability);
}

// ---------------------------------------------------------------------------
// Streamable HTTP cancellation
// ---------------------------------------------------------------------------

/// A published capability generation backed by one in-process Streamable
/// HTTP fixture, built through the production coordinator path.
async fn http_capability(
    tool_runtime: &rustx::tools::runtime::ConversationToolRuntime,
    server_id: &McpServerId,
    binding: rustx::tools::mcp::McpServerBinding,
) -> McpCapability {
    let dir = tempfile::tempdir().expect("capability temp dir");
    let coordinator = CapabilityCoordinator::new(CapabilityCoordinatorConfig {
        python_sources: std::collections::BTreeMap::new(),
        conversation_id: tool_runtime.conversation_id().clone(),
        workspace: tool_runtime.workspace().clone(),
        base_tool_registry: Arc::new(rustx::tools::executor::ToolRegistry::new()),
        extension_tools: tool_runtime.extension_tool_plane(),
        tool_activation: rustx::capabilities::ToolActivationPolicy::default(),
        skill_discovery: rustx::skills::SkillDiscoveryConfig::default(),
        mcp_servers: std::collections::BTreeMap::from([(server_id.clone(), binding)]),
        base_environment: tool_runtime.environment().clone(),
        environment_store_root: dir.path().join("skill-env"),
    })
    .expect("capability coordinator");
    let candidate = coordinator
        .prepare_candidate()
        .await
        .expect("the HTTP fixture publishes its catalog");
    coordinator.commit(candidate).expect("commit generation 1");
    McpCapability {
        coordinator,
        server_id: server_id.clone(),
        _dir: dir,
    }
}

/// Issue #205 review finding 1: a Streamable HTTP `tools/call` that the
/// server accepts and then leaves without **any** response event — so the
/// HTTP response headers themselves are still outstanding — is settled by
/// the MCP executor's own cancellation/settlement plane, not by the generic
/// Issue #204 settlement-control guard.
///
/// # What used to happen
///
/// Post-frontier cancellation awaited `Peer::send_notification` before it
/// could observe the response channel. Over Streamable HTTP an outstanding
/// POST is the client worker's own inline work, so that send could not make
/// progress while this very request was outstanding, and the branch stayed
/// pending indefinitely. The only remaining bound was the generic
/// settlement-control guard — an architecturally wrong place for an MCP
/// transport fact to be caught.
///
/// # Synchronization proof
///
/// - `wait_accepted` resolves only once the server's `tools/call` handler
///   was entered, which is strictly stronger than the effect frontier rustX
///   classifies against, so everything after it is provably post-frontier;
/// - [`streamable_http::TOOL_WITHHOLD`] emits nothing at all, so the server
///   has produced no first message and the POST is genuinely still waiting
///   for its response headers;
/// - the manual clock crosses exactly the hard deadline the generic
///   lifecycle published through its own arming signal;
/// - `wait_terminated` is the **server-side** proof of rustX's local
///   ownership settlement: rmcp's Streamable HTTP server cancels a handler
///   that has emitted nothing when the client disconnects its HTTP request,
///   so this count rises only because rustX actually dropped its in-flight
///   HTTP request and its socket closed.
///
/// The wall-clock value in the deadline policy is never waited on; the only
/// wall clock in this test is `run_mcp_call`'s outer anti-hang guard.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_silent_streamable_http_call_settles_inside_the_mcp_settlement_plane() {
    let fixture = common::native_fixture_without_extensions();
    let server =
        streamable_http::HttpFixture::start(streamable_http::HttpFixtureControl::new()).await;
    let capability = http_capability(
        &fixture.runtime,
        &McpServerId::new("http-recovery"),
        server.binding(),
    )
    .await;
    let lease = capability.coordinator.acquire_attempt_lease();
    let accepted = server.control.clone();
    let audit = run_mcp_call(
        &fixture,
        lease,
        "http-hard-deadline",
        &server.control.withhold(),
        ToolExecutionDeadlinePolicy {
            hard_deadline: Duration::from_mins(1),
            idle_liveness: None,
        },
        move |mut controls| async move {
            controls.wait_started().await;
            // The POST is accepted and in flight, and the server will emit
            // no response event of any kind.
            accepted.wait_accepted(1).await;
            controls.cross_hard_deadline().await;
        },
    )
    .await;

    let result = single_tool_result(&audit);
    let ToolExecutionStatus::OutcomeUnknown { detail } = &result.status else {
        panic!(
            "a dispatched HTTP call with no correlated remote response is unknown, never \
             a proven cancellation or timeout: {:?}",
            result.status
        );
    };
    assert!(
        !detail.is_empty(),
        "the unknown outcome names the frontier it could not cross"
    );
    let facts = execution_facts(&audit);
    assert!(
        !facts.iter().any(|fact| matches!(
            fact,
            RuntimeEvent::ToolExecutionSettlementControlFailed { .. }
        )),
        "the MCP settlement plane returned on its own: the generic control-plane \
         guard is never the bound: {facts:?}"
    );
    assert!(
        !facts.iter().any(|fact| matches!(
            fact,
            RuntimeEvent::ToolExecutionSettlementObserved {
                certainty: rustx::tools::deadline::ToolSettlementCertainty::Confirmed,
                ..
            }
        )),
        "no settlement evidence claims confirmation: {facts:?}"
    );
    assert_eq!(
        facts
            .iter()
            .filter(|fact| matches!(fact, RuntimeEvent::ToolExecutionCompleted { .. }))
            .count(),
        1,
        "exactly one canonical terminal settlement: {facts:?}"
    );
    // The server-side proof that rustX's own local HTTP request ownership is
    // settled: the handler was cancelled by the client's disconnect, which
    // can only happen because the in-flight HTTP request was dropped.
    tokio::time::timeout(Duration::from_secs(30), server.control.wait_terminated(1))
        .await
        .expect("anti-hang guard: the server observes the terminated HTTP request");
    assert_eq!(
        server.control.accepted_calls(),
        1,
        "cancellation never causes a second dispatch"
    );
    drop(capability);
    server.shutdown().await;
}

/// Repeated ambiguous settlements release local progress on the same HTTP
/// generation. Server acceptance and the armed manual deadline order every
/// cancellation; runtime identity proves reconnect/teardown never cleans up.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn repeated_ambiguous_http_calls_release_progress_on_one_live_generation() {
    const CALLS: u32 = 64;
    let fixture = common::native_fixture_without_extensions();
    let server =
        streamable_http::HttpFixture::start(streamable_http::HttpFixtureControl::new()).await;
    let server_id = McpServerId::new("http-progress-boundedness");
    let capability = http_capability(&fixture.runtime, &server_id, server.binding()).await;
    let runtime = capability
        .coordinator
        .current_mcp_runtime(&server_id)
        .expect("published runtime");
    let baseline = runtime.tracked_progress_requests();
    assert_eq!(baseline, 0);
    let ordered = Arc::new(OrderedObservations {
        progress_runtime: Some(Arc::clone(&runtime)),
        ..Default::default()
    });
    let withhold = server.control.withhold();
    let echo = server.control.echo();
    let mut turns = vec![vec![withhold.as_str()]; CALLS as usize];
    turns.push(vec![echo.as_str()]);
    let accepted = server.control.clone();
    let audit = run_mcp_turns(
        &fixture,
        capability.coordinator.acquire_attempt_lease(),
        "http-repeated-ambiguous",
        &turns,
        ToolExecutionDeadlinePolicy {
            hard_deadline: Duration::from_mins(1),
            idle_liveness: None,
        },
        Some(Arc::clone(&ordered)),
        move |controls| async move {
            for count in 1..=CALLS {
                accepted.wait_accepted(count).await;
                // Acceptance is after deadline arming for this call. Each
                // preceding call settled before the next model turn began.
                let deadline = controls.armed.borrow().expect("armed before dispatch");
                assert_eq!(deadline, u64::from(count) * 60_000);
                controls.clock.advance(60_000);
                accepted.wait_terminated(count).await;
            }
        },
    )
    .await;
    let results = tool_results(&audit);
    assert_eq!(results.len(), CALLS as usize + 1);
    assert!(
        results[..CALLS as usize]
            .iter()
            .all(|result| matches!(result.status, ToolExecutionStatus::OutcomeUnknown { .. }))
    );
    assert!(matches!(
        results[CALLS as usize].status,
        ToolExecutionStatus::Success
    ));
    let facts = execution_facts(&audit);
    assert_eq!(
        facts
            .iter()
            .filter(|fact| matches!(fact, RuntimeEvent::ToolExecutionDeadlineFired { .. }))
            .count(),
        CALLS as usize
    );
    assert!(!facts.iter().any(|fact| matches!(
        fact,
        RuntimeEvent::ToolExecutionSettlementControlFailed { .. }
    )));
    assert_eq!(
        *ordered
            .settled_progress_cardinalities
            .lock()
            .expect("settled cardinalities"),
        vec![baseline; CALLS as usize + 1],
        "each terminal publication sees the bounded baseline before the next dispatch"
    );
    assert_eq!(
        ordered.facts(),
        vec![OrderedObservation::Completed; CALLS as usize + 1],
        "no progress and exactly one settlement per call"
    );
    assert_eq!(runtime.outstanding_http_requests(), 0);
    assert_eq!(runtime.tracked_progress_requests(), baseline);
    assert!(
        Arc::ptr_eq(
            &runtime,
            &capability
                .coordinator
                .current_mcp_runtime(&server_id)
                .expect("same healthy runtime")
        ),
        "no reconnect or teardown supplied cleanup"
    );
    assert_eq!(server.control.accepted_calls(), CALLS + 1, "no replay");
    assert_eq!(server.control.terminated_calls(), CALLS);
    drop(capability);
    server.shutdown().await;
}

/// Issue #205 review finding 1, direction B: over Streamable HTTP a
/// **correlated remote response** that won the lifecycle's arbitration stays
/// the terminal winner, and a cancellation issued afterwards cannot reopen
/// it or turn it into an unknown outcome.
///
/// Synchronization proof: the controller waits for the server to accept the
/// call, releases the withheld response, and then waits for the generic
/// lifecycle's physical-settlement pause — reachable only once the MCP
/// executor already produced the remote result. The cancellation is
/// therefore provably post-arbitration, not a timing guess. The server
/// produced the response exactly once and never observed a client
/// disconnect for it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_streamable_http_response_that_won_arbitration_survives_a_later_cancellation() {
    let fixture = common::native_fixture_without_extensions();
    let server =
        streamable_http::HttpFixture::start(streamable_http::HttpFixtureControl::new()).await;
    let capability = http_capability(
        &fixture.runtime,
        &McpServerId::new("http-recovery"),
        server.binding(),
    )
    .await;
    let lease = capability.coordinator.acquire_attempt_lease();
    let control = server.control.clone();
    let audit = run_mcp_call(
        &fixture,
        lease,
        "http-won",
        &server.control.withhold(),
        ToolExecutionDeadlinePolicy {
            hard_deadline: Duration::from_mins(1),
            idle_liveness: None,
        },
        move |mut controls| async move {
            controls.wait_started().await;
            control.wait_accepted(1).await;
            control.release();
            controls.wait_physical_won().await;
            controls.cancellation.cancel();
            controls
                .release_physical
                .send(())
                .expect("the physical settlement pause stays installed");
        },
    )
    .await;

    let result = single_tool_result(&audit);
    assert!(
        matches!(result.status, ToolExecutionStatus::Success),
        "the proven remote HTTP response won arbitration and survives a later \
         cancellation: {:?}",
        result.status
    );
    let facts = execution_facts(&audit);
    assert_eq!(
        facts
            .iter()
            .filter(|fact| matches!(fact, RuntimeEvent::ToolExecutionCompleted { .. }))
            .count(),
        1,
        "exactly one canonical terminal settlement: {facts:?}"
    );
    assert_eq!(server.control.accepted_calls(), 1);
    assert_eq!(
        server.control.terminated_calls(),
        0,
        "a call whose response already won is never terminated as an in-flight \
         HTTP request"
    );
    drop(capability);
    server.shutdown().await;
}

// ---------------------------------------------------------------------------
// Last-known-good carry-forward requires binding identity
// ---------------------------------------------------------------------------

/// The reload inputs of one MCP server identity bound to `binding`.
fn reload_inputs(
    tool_runtime: &rustx::tools::runtime::ConversationToolRuntime,
    server_id: &McpServerId,
    binding: rustx::tools::mcp::McpServerBinding,
) -> rustx::capabilities::CapabilityResourceInputs {
    rustx::capabilities::CapabilityResourceInputs {
        python_sources: std::collections::BTreeMap::new(),
        base_tool_registry: Arc::new(rustx::tools::executor::ToolRegistry::new()),
        tool_activation: rustx::capabilities::ToolActivationPolicy::default(),
        skill_discovery: rustx::skills::SkillDiscoveryConfig::default(),
        mcp_servers: std::collections::BTreeMap::from([(server_id.clone(), binding)]),
        base_environment: tool_runtime.environment().clone(),
    }
}

/// The published MCP tool names of one server in a snapshot.
fn published_mcp_tools(
    snapshot: &Arc<rustx::capabilities::CapabilitySnapshot>,
    server_id: &McpServerId,
) -> Vec<String> {
    snapshot
        .tool_registry()
        .definitions()
        .into_iter()
        .filter(|definition| {
            matches!(
                &definition.origin,
                rustx::tools::types::ToolOrigin::Mcp { server_id: owner } if owner == server_id
            )
        })
        .map(|definition| definition.name)
        .collect()
}

/// Issue #205 review finding 2, direction A: a refresh of the **same**
/// binding that cannot validate keeps the last-known-good capability
/// generation, and the published binding identity is what makes that legal.
///
/// This is the same contract
/// [`a_failed_capability_refresh_keeps_the_last_known_good_generation`]
/// proves end to end; what it adds is the explicit statement of *why* the
/// carry-forward is allowed — the candidate's binding for this server is
/// byte-for-byte the binding the published generation was validated under,
/// so the executors it reuses talk to the transport the published metadata
/// names.
///
/// Synchronization proof: the fixture's generation 2 refuses to handshake,
/// and the parent waits for the server-side `refused:` journal line before
/// asserting, so "the refresh failed" is a fact the child process wrote, not
/// a timing assumption.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn same_binding_carry_forward_keeps_the_published_binding_identity() {
    if recovery::serve_if_recovery_fixture_mode().await {
        return;
    }
    let fixture = common::native_fixture_without_extensions();
    let control = recovery::RecoveryControl::new(fixture.dir().path());
    let script = recovery::RecoveryScript {
        refuse_generations: vec![2],
        ..recovery::RecoveryScript::default()
    };
    let capability = recovery_capability(
        &fixture.runtime,
        "boundary_suites::mcp_recovery::same_binding_carry_forward_keeps_the_published_binding_identity",
        &control,
        &script,
    )
    .await;
    let binding = recovery_binding(
        "boundary_suites::mcp_recovery::same_binding_carry_forward_keeps_the_published_binding_identity",
        &control,
        &script,
    );
    let before = capability.coordinator.current_snapshot();
    let published = published_mcp_tools(&before, &capability.server_id);
    assert_eq!(published.len(), 3, "G1 is the validated catalog");
    assert_eq!(
        before.mcp_servers().get(&capability.server_id),
        Some(&binding),
        "the snapshot freezes the binding its generation was validated under"
    );

    // The reload path with exactly the same binding: the only difference
    // from the published state is that the refresh cannot validate.
    let candidate = capability
        .coordinator
        .prepare_candidate_with_inputs(reload_inputs(
            &fixture.runtime,
            &capability.server_id,
            binding.clone(),
        ))
        .await
        .expect("an unreachable optional MCP source never fails preparation");
    capability
        .coordinator
        .commit(candidate)
        .expect("the carried-forward candidate commits");
    wait_for_journal_entry(&control, &format!("{}2", recovery::JOURNAL_REFUSED_PREFIX));

    let after = capability.coordinator.current_snapshot();
    assert_eq!(
        published_mcp_tools(&after, &capability.server_id),
        published,
        "an unchanged binding may carry its last-known-good catalog forward verbatim"
    );
    assert_eq!(
        after.mcp_servers().get(&capability.server_id),
        Some(&binding),
        "the published binding is unchanged, which is what makes the carry-forward \
         internally coherent"
    );
    assert!(
        matches!(
            capability.coordinator.availability().get(
                &rustx::capabilities::CapabilitySourceId::Mcp(capability.server_id.clone())
            ),
            Some(rustx::capabilities::CapabilitySourceState::Unavailable { .. })
        ),
        "availability and capability knowledge stay separate facts"
    );
    // The retained generation's own live transport still serves calls.
    let result = direct_mcp_call(&fixture, &after, recovery::TOOL_ECHO).await;
    assert!(
        matches!(result.status, ToolExecutionStatus::Success),
        "the carried-forward executor is still usable: {:?}",
        result.status
    );
    assert!(
        result
            .model_facing_projection()
            .as_text()
            .contains("generation 1"),
        "the carried-forward executor talks to the generation the published binding \
         negotiated"
    );
    drop(capability);
}

/// Issue #205 review finding 2, direction B: when the configured binding
/// **changes** and the replacement cannot validate, the previous binding's
/// executors are never published under the new binding's authority.
///
/// # The split-brain this forbids
///
/// ```text
/// published:   S -> B2      (authoritative capability metadata)
/// executor:    S -> connection negotiated from B1
/// ```
///
/// A server id alone is not sufficient identity for the last-known-good
/// fallback: the carried-forward registrations carry their executors, and
/// those executors dispatch through the connection the *published* binding
/// established. Reusing them under a different program, argument,
/// environment, cwd, endpoint, header, or policy would publish metadata that
/// disagrees with the transport that actually runs the call.
///
/// # Synchronization proof
///
/// B1 and B2 differ in an execution-relevant, **server-provable** field: the
/// stdio environment names a different journal file, so which binding
/// spawned a process is a fact the child process itself writes. B2's first
/// generation refuses to handshake, so its refresh deterministically fails,
/// and the parent waits for B2's own `refused:` line before asserting. B1's
/// journal is then checked to have gained nothing — the old binding was
/// never reused to satisfy the new one.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_changed_binding_never_publishes_the_previous_bindings_executors() {
    if recovery::serve_if_recovery_fixture_mode().await {
        return;
    }
    let test_name = "boundary_suites::mcp_recovery::a_changed_binding_never_publishes_the_previous_bindings_executors";
    let fixture = common::native_fixture_without_extensions();
    let first = recovery::RecoveryControl::new(&fixture.dir().path().join("b1"));
    let script = recovery::RecoveryScript::default();
    let capability = recovery_capability(&fixture.runtime, test_name, &first, &script).await;
    let before = capability.coordinator.current_snapshot();
    let published = published_mcp_tools(&before, &capability.server_id);
    assert_eq!(published.len(), 3, "B1/G1 is the authoritative generation");
    let b1_generations = first.established_generations();
    assert_eq!(b1_generations, 1);

    // B2: the same executable, a different execution-relevant environment
    // (its own journal, and a first generation that refuses to handshake),
    // so the replacement provably cannot validate.
    let second = recovery::RecoveryControl::new(&fixture.dir().path().join("b2"));
    let b2_script = recovery::RecoveryScript {
        refuse_generations: vec![1],
        ..recovery::RecoveryScript::default()
    };
    let b2 = recovery_binding(test_name, &second, &b2_script);
    assert_ne!(
        before.mcp_servers().get(&capability.server_id),
        Some(&b2),
        "the test really does change the binding"
    );

    let candidate = capability
        .coordinator
        .prepare_candidate_with_inputs(reload_inputs(
            &fixture.runtime,
            &capability.server_id,
            b2.clone(),
        ))
        .await
        .expect("an unreachable optional MCP source never fails preparation");
    capability
        .coordinator
        .commit(candidate)
        .expect("the replacement candidate commits");
    wait_for_journal_entry(&second, &format!("{}1", recovery::JOURNAL_REFUSED_PREFIX));

    let after = capability.coordinator.current_snapshot();
    assert_eq!(
        after.mcp_servers().get(&capability.server_id),
        Some(&b2),
        "the published binding is the configured desired state"
    );
    assert!(
        published_mcp_tools(&after, &capability.server_id).is_empty(),
        "no executor of B1 is published under B2's authority: {:?}",
        published_mcp_tools(&after, &capability.server_id)
    );
    assert!(
        matches!(
            capability.coordinator.availability().get(
                &rustx::capabilities::CapabilitySourceId::Mcp(capability.server_id.clone())
            ),
            Some(rustx::capabilities::CapabilitySourceState::Unavailable { .. })
        ),
        "B2 is reported unavailable rather than silently satisfied by B1"
    );
    assert_eq!(
        first.established_generations(),
        b1_generations,
        "B1 spawned nothing to serve B2: its journal is untouched"
    );
    assert_eq!(
        first.accepted_calls(recovery::TOOL_ECHO),
        0,
        "no call was routed to B1 under B2's authority"
    );
    drop(capability);
}

/// Issue #205 review finding 2, policy direction: `policy` is part of
/// `McpServerBinding` and part of execution semantics — invocation,
/// concurrency, and approval policy all reach the published
/// `ToolDefinition` — so a policy-only change is a binding change, and a
/// failed refresh of it may not carry the previous generation forward
/// either.
///
/// Synchronization proof: the transport is byte-for-byte identical, so the
/// refresh spawns generation 2 of the same fixture identity, which the
/// script makes refuse; the parent waits for that server-written `refused:`
/// line before asserting.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_policy_only_binding_change_also_forfeits_carry_forward() {
    if recovery::serve_if_recovery_fixture_mode().await {
        return;
    }
    let test_name =
        "boundary_suites::mcp_recovery::a_policy_only_binding_change_also_forfeits_carry_forward";
    let fixture = common::native_fixture_without_extensions();
    let control = recovery::RecoveryControl::new(fixture.dir().path());
    let script = recovery::RecoveryScript {
        refuse_generations: vec![2],
        ..recovery::RecoveryScript::default()
    };
    let capability = recovery_capability(&fixture.runtime, test_name, &control, &script).await;
    let before = capability.coordinator.current_snapshot();
    assert_eq!(published_mcp_tools(&before, &capability.server_id).len(), 3);

    let mut b2 = recovery_binding(test_name, &control, &script);
    b2.policy.approval = rustx::tools::types::ToolApprovalPolicy::Always;
    assert_ne!(
        before.mcp_servers().get(&capability.server_id),
        Some(&b2),
        "an approval-policy change is a binding change"
    );

    let candidate = capability
        .coordinator
        .prepare_candidate_with_inputs(reload_inputs(
            &fixture.runtime,
            &capability.server_id,
            b2.clone(),
        ))
        .await
        .expect("an unreachable optional MCP source never fails preparation");
    capability
        .coordinator
        .commit(candidate)
        .expect("the replacement candidate commits");
    wait_for_journal_entry(&control, &format!("{}2", recovery::JOURNAL_REFUSED_PREFIX));

    let after = capability.coordinator.current_snapshot();
    assert_eq!(after.mcp_servers().get(&capability.server_id), Some(&b2));
    assert!(
        published_mcp_tools(&after, &capability.server_id).is_empty(),
        "tools validated under the previous policy are never republished under a \
         different one: {:?}",
        published_mcp_tools(&after, &capability.server_id)
    );
    drop(capability);
}

// ---------------------------------------------------------------------------
// Progress liveness at concurrency above the old router bound
// ---------------------------------------------------------------------------

/// How many MCP calls the progress-concurrency regression runs at once.
///
/// It is deliberately larger than the router's previous global 16-token
/// cache: at this concurrency that cache was full of live subscriptions, and
/// the progress notification of a call that had not subscribed yet was
/// discarded rather than displacing one — erasing the only liveness evidence
/// that call would ever produce.
const PROGRESS_CONCURRENCY: usize = 20;

/// Issue #205 review finding 3: at a concurrency above the router's previous
/// bound, every admitted in-flight MCP call's genuine remote progress still
/// reaches the generic idle-liveness watchdog, so no call suffers a **false**
/// idle timeout.
///
/// # Synchronization proof
///
/// - all [`PROGRESS_CONCURRENCY`] calls are one parallel batch of a single
///   model turn, so they are in flight simultaneously by construction, not
///   by timing;
/// - `wait_accepted` proves every call reached the server, and every call
///   emitted its dispatch progress notification there;
/// - `wait_progress_at_least` counts *forwarded* progress reports. The
///   progress fanout refreshes the idle watchdog **before** it calls the
///   observer, so observing the Nth report is a happens-after of the Nth
///   idle refresh: the clock advance below can never race an unpublished
///   refresh;
/// - each advance happens only after the previous round of refreshes was
///   observed. The released pulse moves every window to 900..1900, so
///   crossing t=1100 is the discriminator: a call whose progress had been
///   dropped would still hold the window 0..1000 and would fire an idle
///   deadline there.
///
/// ```text
/// t=0     20 dispatch notifications   -> every idle window 0..1000
/// t=900   (advance)                   -> no window has expired
/// t=900   20 released pulses          -> every idle window 900..1900
/// t=1100  (advance)                   -> a dropped-progress call fires Idle here
/// t=1800  (advance)                   -> the immutable hard deadline bounds every call
/// ```
///
/// The 1800ms hard deadline fires while the refreshed idle window is still
/// open, so each call settles as exactly one `OutcomeUnknown` bounded by the
/// hard deadline and never by a false idle timeout.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_mcp_progress_refreshes_every_idle_watchdog_above_the_old_router_bound() {
    let fixture = common::native_fixture_without_extensions();
    let server =
        streamable_http::HttpFixture::start(streamable_http::HttpFixtureControl::new()).await;
    // The batch must actually run in parallel, so the binding's own
    // concurrency policy — an execution-relevant field of the binding — says
    // so for every tool of this server.
    let mut binding = server.binding();
    binding.policy.concurrency = rustx::tools::types::ToolConcurrencyPolicy::Parallel;
    let capability = http_capability(
        &fixture.runtime,
        &McpServerId::new("http-progress"),
        binding,
    )
    .await;
    let lease = capability.coordinator.acquire_attempt_lease();
    let control = server.control.clone();
    let concurrency = u32::try_from(PROGRESS_CONCURRENCY).expect("small concurrency");
    let audit = run_parallel_mcp_calls(
        &fixture,
        lease,
        "http-progress",
        &server.control.pulse(),
        PROGRESS_CONCURRENCY,
        ToolExecutionDeadlinePolicy {
            hard_deadline: Duration::from_millis(1800),
            idle_liveness: Some(Duration::from_secs(1)),
        },
        move |mut controls| async move {
            controls.wait_started().await;
            // Every call reached the server and emitted its dispatch
            // notification there.
            control.wait_accepted(concurrency).await;
            // Every dispatch notification was forwarded, so every call's
            // idle window was refreshed at t=0.
            controls.wait_progress_at_least(concurrency).await;
            // 900 < 1000: every window survives this advance.
            controls.clock.advance(900);
            control.release();
            control.wait_pulsed(concurrency).await;
            // Every released pulse was forwarded, so every window is now
            // 900..1900.
            controls.wait_progress_at_least(concurrency * 2).await;
            // t = 1100: a call whose pulse had been discarded would still
            // hold the window 0..1000 and would fire an idle deadline here.
            controls.clock.advance(200);
            // t = 1800: the immutable hard deadline, still inside every
            // refreshed idle window.
            controls.clock.advance(700);
        },
    )
    .await;

    let facts = execution_facts(&audit);
    let idle_deadlines = facts
        .iter()
        .filter(|fact| {
            matches!(
                fact,
                RuntimeEvent::ToolExecutionDeadlineFired {
                    kind: ToolDeadlineKind::Idle,
                    ..
                }
            )
        })
        .count();
    assert_eq!(
        idle_deadlines, 0,
        "genuine remote progress reached every admitted call's idle watchdog, so no \
         false idle deadline fires"
    );
    assert_eq!(
        facts
            .iter()
            .filter(|fact| matches!(
                fact,
                RuntimeEvent::ToolExecutionDeadlineFired {
                    kind: ToolDeadlineKind::Hard,
                    ..
                }
            ))
            .count(),
        PROGRESS_CONCURRENCY,
        "the immutable hard deadline is what bounds every call"
    );
    let results = tool_results(&audit);
    assert_eq!(
        results.len(),
        PROGRESS_CONCURRENCY,
        "every call settles exactly once, canonically"
    );
    for result in &results {
        assert!(
            matches!(result.status, ToolExecutionStatus::OutcomeUnknown { .. }),
            "the hard deadline bounds a call with no correlated remote response: {:?}",
            result.status
        );
    }
    let progress_facts = audit
        .event_history
        .iter()
        .filter(|event| matches!(event, RuntimeEvent::ToolExecutionProgress { .. }))
        .count();
    assert_eq!(
        progress_facts,
        PROGRESS_CONCURRENCY * 2,
        "every remote pulse of every concurrent call was forwarded as durable \
         liveness evidence; none was discarded and none was fabricated"
    );
    drop(capability);
    server.shutdown().await;
}

/// Issue #205 review finding, end to end: a correlated remote response never
/// deletes progress the dispatching `ToolCall` has not claimed yet, and that
/// progress is still reported **before** the terminal `ToolResult`.
///
/// The reachable ordering this pins is the one the executor cannot design
/// away: rmcp mints a request's progress token *inside* the request, so a
/// call is unavoidably between its effect frontier and `progress.subscribe`
/// for a while, and a fast server can emit genuine progress *and* answer
/// inside that window. The reviewed implementation treated that answer as
/// proof that no caller remained, deleted the buffered occurrence, and let
/// the biased response arm report a terminal result with the liveness
/// evidence silently gone.
///
/// # Synchronization proof
///
/// Every claimed interleaving is a fact this test waits for; the only wall
/// clock is the suite's anti-hang guard, and the manual clock never moves.
///
/// 1. **the call is admitted and past its effect frontier.**
///    `ProgressSubscriptionRace` parks it at the exact instant after
///    `send_cancellable_request` returned `Ok` — where its progress lease
///    already exists — and before `McpProgressLease::subscribe`;
/// 2. **no subscription exists.** `wait_parked(1)` resolves only inside that
///    window;
/// 3. **the server ran.** `wait_accepted(1)` is the server's own fact, and
///    [`streamable_http::TOOL_ANNOUNCE`] publishes it *after* its progress
///    notification is on this request's own ordered stream and *before* it
///    returns the correlated result;
/// 4. **the progress reached the router with no subscriber.**
///    `wait_pre_subscription_progress(1)` resolves only once the router has
///    recorded a delivery for this request's own token that landed while
///    that token had no subscriber;
/// 5. **the correlated response reached the inbound seam.**
///    `wait_answered(1)` resolves only once `ObservingTransport::receive`
///    has correlated the response to this request id — the exact instant at
///    which the reviewed implementation deleted the evidence;
/// 6. **the caller is still parked, and still owns its evidence.**
///    `every_parked_request_retains_its_progress()` is a settled fact, not a
///    sample: a parked call can neither subscribe nor relinquish, so nothing
///    can change it while the assertion runs. This is the assertion that
///    fails on the reviewed implementation;
/// 7. only then is the call released, so it subscribes *after* its response
///    is already resolved and claims the buffered occurrence.
///
/// The ordering claim itself is read from
/// [`OrderedObservations`], written on the runtime's own publication path —
/// the live progress seam and the terminal `ToolExecutionCompleted` fact —
/// so `[Progress, Completed]` is a happens-before, not a vector-position
/// inference.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_correlated_response_never_deletes_progress_the_caller_has_not_claimed() {
    let fixture = common::native_fixture_without_extensions();
    let server =
        streamable_http::HttpFixture::start(streamable_http::HttpFixtureControl::new()).await;
    let capability = http_capability(
        &fixture.runtime,
        &McpServerId::new("http-progress-retention"),
        server.binding(),
    )
    .await;
    let lease = capability.coordinator.acquire_attempt_lease();
    let announce = server.control.announce();
    let control = server.control.clone();
    let (race, _race_guard) =
        rustx::tools::mcp::test_sync::ProgressSubscriptionRace::install(&announce);
    let parking = Arc::clone(&race);
    let ordered = Arc::new(OrderedObservations::default());
    let audit = run_mcp_call_ordered(
        &fixture,
        lease,
        "http-progress-retention",
        &announce,
        ToolExecutionDeadlinePolicy {
            hard_deadline: Duration::from_mins(10),
            idle_liveness: Some(Duration::from_secs(1)),
        },
        &ordered,
        move |mut controls| async move {
            controls.wait_started().await;
            // The call has crossed its effect frontier and holds its progress
            // lease; it has not subscribed.
            parking.wait_parked(1).await;
            // The server entered its handler, having already emitted its
            // progress notification.
            control.wait_accepted(1).await;
            // That progress reached the router while the token had no
            // subscriber.
            parking.wait_pre_subscription_progress(1).await;
            // And the correlated response reached the inbound seam.
            parking.wait_answered(1).await;
            assert_eq!(
                parking.parked_requests(),
                1,
                "the dispatching call is still inside the pre-subscription window"
            );
            assert!(
                parking.every_parked_request_retains_its_progress(),
                "a correlated response ends remote execution; it does not end the \
                 dispatching call's progress ownership, so the observed occurrence \
                 is still there to be claimed"
            );
            // Only now may the call subscribe — after its response is
            // already resolved.
            parking.release();
            // The claimed occurrence was forwarded through the generic
            // progress seam.
            controls.wait_progress_at_least(1).await;
        },
    )
    .await;

    assert!(
        race.every_parked_request_reached_its_forget_point(),
        "the call claimed its buffered evidence and then relinquished it, and the \
         answered request was forgotten: no progress state outlives the ToolCall"
    );
    assert_eq!(
        ordered.facts(),
        vec![OrderedObservation::Progress, OrderedObservation::Completed],
        "genuine progress observed before the correlated response is reported \
         before the terminal ToolResult, even though it was claimed after that \
         response had already resolved"
    );
    let facts = execution_facts(&audit);
    assert_eq!(
        facts
            .iter()
            .filter(|fact| matches!(fact, RuntimeEvent::ToolExecutionCompleted { .. }))
            .count(),
        1,
        "the accepted ToolCall settles exactly once"
    );
    assert_eq!(
        facts
            .iter()
            .filter(|fact| matches!(fact, RuntimeEvent::ToolExecutionDeadlineFired { .. }))
            .count(),
        0,
        "no deadline participates in this ordering"
    );
    let result = single_tool_result(&audit);
    assert!(
        matches!(result.status, ToolExecutionStatus::Success),
        "the correlated response is the call's outcome: {:?}",
        result.status
    );
    assert_eq!(
        audit
            .event_history
            .iter()
            .filter(|event| matches!(event, RuntimeEvent::ToolExecutionProgress { .. }))
            .count(),
        1,
        "the one genuine remote occurrence is durable, and none is fabricated"
    );
    drop(capability);
    server.shutdown().await;
}

/// Issue #205 review finding 1, drain direction: the per-request local HTTP
/// control primitives this contract introduces are owned by connection
/// close, so none of them can survive a drain.
///
/// # Synchronization proof
///
/// The call and the close run as two arms of one `join!`, so no task is
/// detached and no ordering is guessed. `wait_accepted` gates the close on
/// the server having entered its handler — the request is provably in flight
/// and its response headers are provably still outstanding, because
/// [`streamable_http::TOOL_WITHHOLD`] emits nothing. `wait_terminated` is the
/// server-side proof that the close actually terminated rustX's in-flight
/// HTTP request rather than abandoning it, and `close` returning `Ok` is the
/// runtime's own physical settlement proof.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn drain_terminates_every_streamable_http_request_the_generation_still_owns() {
    struct NoProgress;
    impl rustx::tools::executor::ProgressReporter for NoProgress {
        fn report(&self, _progress: rustx::tools::types::ToolProgress) {}
    }

    let fixture = common::native_fixture_without_extensions();
    let server =
        streamable_http::HttpFixture::start(streamable_http::HttpFixtureControl::new()).await;
    // A directly connected runtime: its connection is *fixed*, so this test
    // owns the runtime and closes it itself — exactly the drain boundary
    // under test, with no capability generation in the way.
    let runtime = rustx::tools::mcp::McpServerRuntime::connect(
        &McpServerId::new("http-drain"),
        &server.binding(),
        fixture.runtime.workspace(),
        Arc::new(rustx::tools::mcp::McpInvalidationState::new()),
    )
    .await
    .expect("HTTP MCP connect");
    let executor =
        rustx::tools::mcp::McpToolExecutor::new(Arc::clone(&runtime), server.control.withhold());
    let progress = NoProgress;
    let context = rustx::tools::executor::ToolExecutionContext::new(
        fixture.runtime.conversation_id(),
        None,
        rustx::runtime::ExecutionCancellation::detached(
            rustx::runtime::CancellationSignal::new(),
            CancellationReason::UserRequested,
        ),
        fixture.runtime.workspace(),
        &progress,
        fixture.runtime.artifacts(),
        fixture.runtime.tool_output(),
        fixture.runtime.environment(),
    );
    let invocation = rustx::tools::types::ToolInvocation {
        id: rustx::tools::types::ToolInvocationId::Agent {
            call_id: rustx::runtime::identity::ToolCallId::new("http-drain-call"),
        },
        tool_id: rustx::runtime::identity::ToolId::new("http-drain-tool"),
        tool_name: server.control.withhold(),
        mode: rustx::tools::types::ToolInvocationMode::Foreground,
        arguments: serde_json::json!({}),
    };

    let control = server.control.clone();
    let (result, settlement) = tokio::time::timeout(
        Duration::from_mins(1),
        futures_util::future::join(
            rustx::tools::executor::ToolExecutor::start(&executor, invocation, context).completion,
            async {
                control.wait_accepted(1).await;
                runtime.close().await
            },
        ),
    )
    .await
    .expect("anti-hang guard: drain always settles an in-flight HTTP request");

    settlement.expect("the connection proves physical settlement");
    assert!(
        !matches!(result.status, ToolExecutionStatus::Success),
        "a request drained mid-flight never produces a successful remote result: {:?}",
        result.status
    );
    // The server-side proof: the in-flight HTTP request was terminated by
    // drain, not left running past it.
    tokio::time::timeout(Duration::from_secs(30), server.control.wait_terminated(1))
        .await
        .expect("anti-hang guard: drain terminates the owned HTTP request");
    assert_eq!(server.control.accepted_calls(), 1);
    server.shutdown().await;
}

/// How many concurrent in-flight MCP calls the pre-subscription regression
/// holds open at once.
///
/// It is deliberately far above the router's removed 32-entry
/// pre-subscription window. Nothing in the architecture bounded how many
/// admitted requests could sit inside that window simultaneously, which is
/// precisely why bounding it by capacity was the wrong shape: past 32, a
/// legitimate live request's only liveness evidence became an eviction
/// candidate.
const PRE_SUBSCRIPTION_CONCURRENCY: usize = 64;

/// Issue #205 review finding: genuine remote progress that arrives **before**
/// the dispatching executor has completed its subscription registration is
/// never lost, at a concurrency far above the removed pre-subscription
/// window, and therefore never turns into a false idle timeout.
///
/// This is the end-to-end shape of the hole. Every one of these calls is a
/// real MCP `tools/call` over a real Streamable HTTP transport, admitted by
/// the generic Issue #204 lifecycle, and every one of them has its server
/// answer with genuine progress while it is still inside the window that
/// used to be a bounded FIFO cache.
///
/// # Synchronization proof
///
/// Every claimed interleaving is proven by a rendezvous, never by a sleep;
/// the only wall clock is the suite's outer anti-hang guard.
///
/// 1. **every call is admitted and crosses its dispatch frontier.** The
///    [`PRE_SUBSCRIPTION_CONCURRENCY`] calls are one parallel batch of a
///    single model turn, so they are in flight simultaneously by
///    construction. `ProgressSubscriptionRace` parks each one at the exact
///    instant *after* `send_cancellable_request` returned `Ok` — the effect
///    frontier — and *before* `McpProgressRouter::subscribe`;
/// 2. **no subscription exists yet.** `wait_parked(N)` resolves only when
///    all N calls are simultaneously inside that window. A parked call has
///    provably not registered its subscription;
/// 3. **every server emitted genuine progress.** `wait_accepted(N)` is the
///    server's own fact: [`streamable_http::TOOL_PULSE`] emits its dispatch
///    notification *before* it publishes the acceptance;
/// 4. **that progress reached the MCP client before the subscription.**
///    `wait_pre_subscription_progress(N)` resolves only once the router has
///    recorded, for every parked request's own progress token, a delivery
///    that landed while that token had no subscriber. That is the claimed
///    interleaving stated exactly, per request, with no timing inference;
/// 5. **no request completes yet.** `TOOL_PULSE` never answers.
///
/// Only then is the race released, every call subscribes and claims the
/// evidence the router held for it, and the manual clock advances.
///
/// ```text
/// t=0    N calls parked pre-subscription, N genuine progress notifications
///        already delivered to the router and owned by their requests
/// t=0    release -> every call subscribes and claims its own evidence
///                -> every idle window becomes 0..1000
/// t=900  (advance)  no window has expired
/// t=900  N released pulses -> every idle window becomes 900..1900
/// t=1100 (advance)  a call whose pre-subscription evidence had been evicted
///                   would still hold the window 0..1000 and fire Idle here
/// t=1800 (advance)  the immutable hard deadline bounds every call
/// ```
///
/// Under the removed bound this test could not pass: 64 requests share a
/// 32-entry FIFO, so 32 of them would have had their only progress
/// occurrence evicted by another *legitimate* request before they could
/// claim it, and would have fired a false idle deadline at t=1100.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn progress_that_beats_every_subscription_still_refreshes_every_idle_watchdog() {
    let fixture = common::native_fixture_without_extensions();
    let server =
        streamable_http::HttpFixture::start(streamable_http::HttpFixtureControl::new()).await;
    let mut binding = server.binding();
    binding.policy.concurrency = rustx::tools::types::ToolConcurrencyPolicy::Parallel;
    let capability = http_capability(
        &fixture.runtime,
        &McpServerId::new("http-pre-subscription"),
        binding,
    )
    .await;
    let lease = capability.coordinator.acquire_attempt_lease();
    let control = server.control.clone();
    let concurrency = u32::try_from(PRE_SUBSCRIPTION_CONCURRENCY).expect("small concurrency");
    // Installed before the batch starts, and uninstalled with the guard, so
    // no other suite in this binary can be parked by it: the race is scoped
    // to this one tool name.
    let (race, _race_guard) =
        rustx::tools::mcp::test_sync::ProgressSubscriptionRace::install(&server.control.pulse());
    let parking = Arc::clone(&race);
    let audit = run_parallel_mcp_calls(
        &fixture,
        lease,
        "http-pre-subscription",
        &server.control.pulse(),
        PRE_SUBSCRIPTION_CONCURRENCY,
        ToolExecutionDeadlinePolicy {
            hard_deadline: Duration::from_millis(1800),
            idle_liveness: Some(Duration::from_secs(1)),
        },
        move |mut controls| async move {
            controls.wait_started().await;
            // Every call is admitted and has crossed its effect frontier,
            // and none of them has registered a subscription.
            parking.wait_parked(PRE_SUBSCRIPTION_CONCURRENCY).await;
            // Every server entered its handler, and emitted its dispatch
            // progress notification before publishing that fact.
            control.wait_accepted(concurrency).await;
            // The claim under test: for every one of those still-parked
            // requests, its own progress reached the MCP client while it had
            // no subscriber.
            parking
                .wait_pre_subscription_progress(PRE_SUBSCRIPTION_CONCURRENCY)
                .await;
            assert_eq!(
                parking.parked_requests(),
                PRE_SUBSCRIPTION_CONCURRENCY,
                "every request is still inside the pre-subscription window"
            );
            // Only now may the calls subscribe and claim the evidence the
            // router owned on their behalf.
            parking.release();
            // Every claimed notification was forwarded, so every call's idle
            // window was refreshed at t=0.
            controls.wait_progress_at_least(concurrency).await;
            // 900 < 1000: every window survives this advance.
            controls.clock.advance(900);
            control.release();
            control.wait_pulsed(concurrency).await;
            // Every released pulse was forwarded, so every window is now
            // 900..1900.
            controls.wait_progress_at_least(concurrency * 2).await;
            // t = 1100: a call whose pre-subscription evidence had been
            // evicted would still hold the window 0..1000 and fire Idle here.
            controls.clock.advance(200);
            // t = 1800: the immutable hard deadline, still inside every
            // refreshed idle window.
            controls.clock.advance(700);
        },
    )
    .await;

    let facts = execution_facts(&audit);
    assert_eq!(
        facts
            .iter()
            .filter(|fact| matches!(
                fact,
                RuntimeEvent::ToolExecutionDeadlineFired {
                    kind: ToolDeadlineKind::Idle,
                    ..
                }
            ))
            .count(),
        0,
        "progress that beat every subscription still reached every admitted call's \
         idle watchdog, so no false idle deadline fires"
    );
    assert_eq!(
        facts
            .iter()
            .filter(|fact| matches!(
                fact,
                RuntimeEvent::ToolExecutionDeadlineFired {
                    kind: ToolDeadlineKind::Hard,
                    ..
                }
            ))
            .count(),
        PRE_SUBSCRIPTION_CONCURRENCY,
        "the immutable hard deadline is what bounds every call"
    );
    let results = tool_results(&audit);
    assert_eq!(
        results.len(),
        PRE_SUBSCRIPTION_CONCURRENCY,
        "every call settles exactly once, canonically"
    );
    for result in &results {
        assert!(
            matches!(result.status, ToolExecutionStatus::OutcomeUnknown { .. }),
            "the hard deadline bounds a call with no correlated remote response: {:?}",
            result.status
        );
    }
    assert_eq!(
        audit
            .event_history
            .iter()
            .filter(|event| matches!(event, RuntimeEvent::ToolExecutionProgress { .. }))
            .count(),
        PRE_SUBSCRIPTION_CONCURRENCY * 2,
        "every remote pulse of every concurrent call was forwarded as durable \
         liveness evidence; none was discarded and none was fabricated"
    );
    drop(capability);
    server.shutdown().await;
}

/// Issue #205 review finding: a normal successful Streamable HTTP request
/// cleans up its own lifecycle state, so the ownership registry's memory
/// bound is the in-flight request count and never the count of requests the
/// connection generation has ever served.
///
/// # Synchronization proof
///
/// No race and no clock: each call is awaited to its canonical terminal
/// result before the next begins, and every one of them is a complete
/// success — the POST registered, the response body was consumed, and the
/// correlated remote response was delivered. The registry is then read
/// **without closing the connection**, which is the whole point: close is
/// not allowed to be the thing that cleans up.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn successful_streamable_http_requests_leave_no_request_lifecycle_state() {
    let fixture = common::native_fixture_without_extensions();
    let server =
        streamable_http::HttpFixture::start(streamable_http::HttpFixtureControl::new()).await;
    let runtime = rustx::tools::mcp::McpServerRuntime::connect(
        &McpServerId::new("http-cleanup"),
        &server.binding(),
        fixture.runtime.workspace(),
        Arc::new(rustx::tools::mcp::McpInvalidationState::new()),
    )
    .await
    .expect("HTTP MCP connect");
    let echo = server.control.echo();
    let executor = rustx::tools::mcp::McpToolExecutor::new(Arc::clone(&runtime), echo.clone());

    let release = runtime.hold_http_release("tools/call");
    for index in 0..32 {
        let call = direct_executor_call(&fixture, &executor, &echo, index);
        tokio::pin!(call);
        if index == 0 {
            tokio::select! {
                result = &mut call => panic!("response settled before HTTP release: {result:?}; {:?}", runtime.http_request_states()),
                () = release.wait_held_and_observed() => {}
            }
            assert_eq!(
                runtime.http_request_states(),
                vec![(
                    release.request_id(),
                    Some("tools/call".to_owned()),
                    "HttpOwned".to_owned(),
                    true
                )]
            );
            assert!(futures_util::FutureExt::now_or_never(call.as_mut()).is_none());
            release.release();
        }
        let result = tokio::time::timeout(Duration::from_mins(1), &mut call)
            .await
            .expect("anti-hang guard: an answered HTTP call settles");
        assert!(
            matches!(result.status, ToolExecutionStatus::Success),
            "call {index} is an ordinary success: {:?}",
            result.status
        );
        assert_eq!(
            runtime.outstanding_http_requests(),
            0,
            "call {index} must forget its exact request before settlement: {:?}",
            runtime.http_request_states()
        );
    }
    // ...and it is still empty after the last one, with the connection open:
    // close is not allowed to be the thing that cleans up.
    assert_eq!(
        runtime.outstanding_http_requests(),
        0,
        "every completed request forgot its own lifecycle state before close"
    );
    runtime.close().await.expect("physical settlement");
    server.shutdown().await;
}

/// Executes one MCP tool directly through the executor boundary with an
/// uncancelled context, and returns its canonical result.
async fn direct_executor_call(
    fixture: &common::NativeFixture,
    executor: &rustx::tools::mcp::McpToolExecutor,
    tool_name: &str,
    index: usize,
) -> rustx::tools::types::ToolExecutionResult {
    struct NoProgress;
    impl rustx::tools::executor::ProgressReporter for NoProgress {
        fn report(&self, _progress: rustx::tools::types::ToolProgress) {}
    }

    let progress = NoProgress;
    let context = rustx::tools::executor::ToolExecutionContext::new(
        fixture.runtime.conversation_id(),
        None,
        rustx::runtime::ExecutionCancellation::detached(
            rustx::runtime::CancellationSignal::new(),
            CancellationReason::UserRequested,
        ),
        fixture.runtime.workspace(),
        &progress,
        fixture.runtime.artifacts(),
        fixture.runtime.tool_output(),
        fixture.runtime.environment(),
    );
    let invocation = rustx::tools::types::ToolInvocation {
        id: rustx::tools::types::ToolInvocationId::Agent {
            call_id: rustx::runtime::identity::ToolCallId::new(format!(
                "http-cleanup-call-{index}"
            )),
        },
        tool_id: rustx::runtime::identity::ToolId::new("http-cleanup-tool"),
        tool_name: tool_name.to_owned(),
        mode: rustx::tools::types::ToolInvocationMode::Foreground,
        arguments: serde_json::json!({}),
    };
    rustx::tools::executor::ToolExecutor::start(executor, invocation, context)
        .completion
        .await
}

/// Issue #205 review finding: a Streamable HTTP request cancelled **before**
/// its outbound participant consumed the termination never reaches the
/// server, settles at the correct local ownership point, and leaves no
/// request lifecycle state behind.
///
/// This is the end-to-end shape, over the real transport, through the
/// generic #204 lifecycle, against a real server that can say whether it ever
/// saw the call.
///
/// # Synchronization proof
///
/// - `OutboundDispatchPause` parks the outbound send of exactly this
///   fixture's tool after `Transport::send`'s synchronous prologue took
///   dispatch ownership and before the inner transport can be polled. Tool
///   names are minted per fixture instance, so no other test in this binary
///   can be parked by it and it can never park another test's calls;
/// - `wait_parked(1)` resolves only once that participant is actually
///   parked, so everything after it is provably inside that window;
/// - `cross_hard_deadline` advances the manual clock across exactly the
///   deadline the generic lifecycle published through its own arming signal,
///   so the cancellation intent lands while the participant is parked;
/// - `wait_decided(1)` is the participant's **own** recorded decision. The
///   controller releases the pause and then waits for that fact, so the
///   assertions below are about a continuation that provably ran — not about
///   one that merely had permission to;
/// - `refusals() == 1` and `dispatches() == 0` say what it decided;
/// - `accepted_calls() == 0` is the server's own fact that the request never
///   reached it. This fixture counts every tool it enters, so that count is
///   evidence rather than a tool that never counted.
///
/// The only wall clock is the outer anti-hang guard.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_streamable_http_request_cancelled_before_dispatch_never_reaches_the_server() {
    let fixture = common::native_fixture_without_extensions();
    let server =
        streamable_http::HttpFixture::start(streamable_http::HttpFixtureControl::new()).await;
    let capability = http_capability(
        &fixture.runtime,
        &McpServerId::new("http-pre-dispatch"),
        server.binding(),
    )
    .await;
    let lease = capability.coordinator.acquire_attempt_lease();
    // Installed after discovery, so only the tool call is parked: the
    // handshake and `tools/list` are not tool invocations and are never
    // tracked by this seam at all.
    let echo = server.control.echo();
    let (pause, _pause_guard) = rustx::tools::mcp::test_sync::OutboundDispatchPause::install(&echo);
    let parking = Arc::clone(&pause);
    let audit = run_mcp_call(
        &fixture,
        lease,
        "http-pre-dispatch",
        &echo,
        ToolExecutionDeadlinePolicy {
            hard_deadline: Duration::from_millis(1500),
            idle_liveness: None,
        },
        move |mut controls| async move {
            controls.wait_started().await;
            // The request has an outbound participant that owns it and has
            // not reached the transport.
            parking.wait_parked(1).await;
            // The cancellation intent lands inside that window.
            controls.cross_hard_deadline().await;
            // ...and the MCP settlement plane has *applied* it to this exact
            // request. Advancing a clock only causes a cancellation; the
            // participant must find a termination that already happened.
            parking.wait_terminated(1).await;
            // The parked participant may now run. Settlement cannot complete
            // until it has consumed the termination, so this release is a
            // precondition of the attempt finishing at all.
            parking.release();
            // Its own decision, waited for rather than assumed.
            parking.wait_decided(1).await;
        },
    )
    .await;

    assert_eq!(
        pause.refusals(),
        1,
        "the outbound participant observed the terminal lifecycle state and refused"
    );
    assert_eq!(
        pause.dispatches(),
        0,
        "no participant of a terminated invocation handed it to the transport"
    );
    // The call settled inside the MCP settlement plane: no generic
    // settlement-control failure had to bound it.
    let facts = execution_facts(&audit);
    assert!(
        !facts.iter().any(|fact| matches!(
            fact,
            RuntimeEvent::ToolExecutionSettlementControlFailed { .. }
        )),
        "the MCP executor settled the call itself: {facts:?}"
    );
    let result = single_tool_result(&audit);
    assert!(
        matches!(result.status, ToolExecutionStatus::OutcomeUnknown { .. }),
        "a request that crossed its effect frontier settles as OutcomeUnknown: {:?}",
        result.status
    );
    // The server's own fact: the request was refused before the transport and
    // never reached it, so no remote side effect happened after the terminal
    // result.
    assert_eq!(
        server.control.accepted_calls(),
        0,
        "a request terminated before its dispatch never reaches the server"
    );
    drop(capability);
    server.shutdown().await;
}

/// Issue #205 review finding, stated as the ordering it is about: **a
/// `ToolCall` that has reached terminal settlement can have no local
/// participant left that is able to dispatch it.**
///
/// # The interleaving this pins
///
/// rmcp's service loop *calls* `Transport::send` and only then spawns the
/// returned future. Those are two instants, and the bug lived between them:
///
/// ```text
/// send_cancellable_request -> Ok        the request is on the peer queue
/// admit(id)                             lifecycle entry exists
/// Transport::send(..) called            the send future exists, unpolled
/// hard deadline wins                    terminate() -> "nothing pending"
/// settled + reported                    canonical terminal ToolResult
/// admission dropped                     the entry is forgotten
/// the send future is finally polled     entry(id).or_insert_with(..)
///                                       -> a FRESH, UNCANCELLED entry
/// tools/call reaches the server         AFTER its terminal result
/// ```
///
/// # Synchronization proof
///
/// Every step below is a fact this test waits for, and nothing is inferred
/// from having released something:
///
/// 1. the call is dispatched through the real executor boundary, so its
///    effect frontier is crossed;
/// 2. `wait_parked(1)` proves rmcp called `ObservingTransport::send` for this
///    request *and* that its participant is parked before the inner
///    transport;
/// 3. cancellation wins while it is parked, and `wait_terminated(1)` proves
///    the MCP settlement plane applied it to this exact request — a
///    cancellation token is level-triggered, so that proof cannot be missed;
/// 4. `wait_awaiting_local_proof(1)` proves the MCP settlement plane has
///    finished everything except the local-ownership proof — the
///    best-effort `notifications/cancelled` and the response arbitration are
///    over, so nothing that remains involves the network. The operation
///    future is then polled to exhaustion of the runtime's ready work and
///    **stays pending**: settlement cannot complete while a participant that
///    can still dispatch has not consumed the termination. This is the
///    assertion that fails on the previous implementation, which had nothing
///    to wait for at this point;
/// 5. the participant is released, and `wait_decided(1)` is its own recorded
///    decision;
/// 6. `refusals() == 1` says it refused rather than dispatched;
/// 7. only now does the tool result exist, and there is exactly one;
/// 8. `accepted_calls() == 0` is the server's proof that the refused request
///    never reached it;
/// 9. the connection is **kept open** and a later independent call succeeds
///    on it, so the result above is the lifecycle refusing one request id —
///    not teardown suppressing a stale send.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_terminal_tool_call_never_dispatches_from_its_stale_outbound_send() {
    struct NoProgress;
    impl rustx::tools::executor::ProgressReporter for NoProgress {
        fn report(&self, _progress: rustx::tools::types::ToolProgress) {}
    }

    let fixture = common::native_fixture_without_extensions();
    let server =
        streamable_http::HttpFixture::start(streamable_http::HttpFixtureControl::new()).await;
    // A directly connected runtime: this test owns the operation future, so
    // it can prove settlement is *pending* rather than only observe what it
    // eventually became.
    let runtime = rustx::tools::mcp::McpServerRuntime::connect(
        &McpServerId::new("http-stale-send"),
        &server.binding(),
        fixture.runtime.workspace(),
        Arc::new(rustx::tools::mcp::McpInvalidationState::new()),
    )
    .await
    .expect("HTTP MCP connect");
    let echo = server.control.echo();
    let executor = rustx::tools::mcp::McpToolExecutor::new(Arc::clone(&runtime), echo.clone());
    let (pause, _pause_guard) = rustx::tools::mcp::test_sync::OutboundDispatchPause::install(&echo);

    let signal = rustx::runtime::CancellationSignal::new();
    let progress = NoProgress;
    let context = rustx::tools::executor::ToolExecutionContext::new(
        fixture.runtime.conversation_id(),
        None,
        rustx::runtime::ExecutionCancellation::detached(
            signal.clone(),
            CancellationReason::UserRequested,
        ),
        fixture.runtime.workspace(),
        &progress,
        fixture.runtime.artifacts(),
        fixture.runtime.tool_output(),
        fixture.runtime.environment(),
    );
    let invocation = rustx::tools::types::ToolInvocation {
        id: rustx::tools::types::ToolInvocationId::Agent {
            call_id: rustx::runtime::identity::ToolCallId::new("http-stale-send-call"),
        },
        tool_id: rustx::runtime::identity::ToolId::new("http-stale-send-tool"),
        tool_name: echo.clone(),
        mode: rustx::tools::types::ToolInvocationMode::Foreground,
        arguments: serde_json::json!({}),
    };
    let completion =
        rustx::tools::executor::ToolExecutor::start(&executor, invocation, context).completion;
    tokio::pin!(completion);

    // (2) rmcp has called `Transport::send` for this request and its
    // participant is parked before the inner transport.
    //
    // The operation future is driven by this task, so this rendezvous is
    // awaited *while polling it* — and a call that settled early would be
    // caught here rather than silently satisfy a later assertion. Racing the
    // two is sound at this point and at (3): settlement there would require a
    // termination that has not been requested yet, so the competing arm
    // cannot legitimately become ready at all. At (5) it can, which is why
    // that step does not race.
    tokio::time::timeout(Duration::from_mins(1), async {
        tokio::select! {
            biased;
            () = pause.wait_parked(1) => {}
            _ = completion.as_mut() => {
                panic!("the call settled before its outbound participant reached the seam")
            }
        }
    })
    .await
    .expect("anti-hang guard: the outbound participant parks at the seam");

    // (3) The cancellation wins while the participant is parked, and the MCP
    // settlement plane applies it to this exact request.
    signal.cancel();
    tokio::time::timeout(Duration::from_mins(1), async {
        tokio::select! {
            biased;
            () = pause.wait_terminated(1) => {}
            _ = completion.as_mut() => {
                panic!("the call settled before its termination was applied")
            }
        }
    })
    .await
    .expect("anti-hang guard: the termination is applied to the parked request");

    // (4) The MCP settlement plane has finished everything it can before the
    // local-ownership proof: the best-effort `notifications/cancelled` was
    // raced and the response channel arbitrated. This is the discriminating
    // ordering point — everything before it is real network work whose
    // duration bounds nothing, and everything after it is one `await` on the
    // release proof plus building a result.
    tokio::time::timeout(Duration::from_mins(1), async {
        tokio::select! {
            biased;
            () = pause.wait_awaiting_local_proof(1) => {}
            _ = completion.as_mut() => {
                panic!("the call settled before its settlement plane reached the local proof")
            }
        }
    })
    .await
    .expect("anti-hang guard: the settlement plane reaches the local-ownership proof");

    // Settlement still cannot complete, because a participant that can still
    // dispatch has not consumed the termination. Yielding hands the runtime
    // every task that is ready, so a future still pending afterwards is
    // pending because nothing released it — not because it has not been
    // scheduled. An implementation that read the pre-dispatch phase as
    // "nothing pending" resolves here within a poll or two.
    for _ in 0..256 {
        assert!(
            futures_util::poll!(completion.as_mut()).is_pending(),
            "a ToolCall must not settle while an outbound participant of the same request \
             is still capable of dispatching it"
        );
        tokio::task::yield_now().await;
    }
    assert_eq!(
        pause.decisions(),
        0,
        "the participant has not consumed the termination yet"
    );

    // (5) Release it, and wait for its own recorded decision.
    //
    // Deliberately *not* raced against the operation future. The refusal
    // publishes this request's release proof, so once the participant has
    // decided the settlement may complete at any instant — and a
    // `select!` polls its arms in sequence, so an arm that was Pending when
    // it was polled can become ready before the next arm is. Racing them
    // here would test the scheduler, not the ordering. The ordering proof is
    // step (4): settlement is provably pending for as long as the
    // participant has not decided. The operation needs no driving here — the
    // outbound participant is rmcp's own task — and it is awaited below.
    pause.release();
    tokio::time::timeout(Duration::from_mins(1), pause.wait_decided(1))
        .await
        .expect("anti-hang guard: the released participant reaches its decision");
    // (6) It refused, rather than dispatching after a terminal settlement.
    assert_eq!(pause.refusals(), 1, "the outbound participant refused");
    assert_eq!(pause.dispatches(), 0, "and dispatched nothing");

    // (7) Exactly one canonical terminal result exists for the cancelled
    // call, and only now.
    let result = tokio::time::timeout(Duration::from_mins(1), completion)
        .await
        .expect("anti-hang guard: the refusal releases the settlement");
    assert!(
        matches!(
            result.status,
            ToolExecutionStatus::OutcomeUnknown { .. } | ToolExecutionStatus::Cancelled { .. }
        ),
        "the cancelled call settles terminally exactly once: {:?}",
        result.status
    );

    // (8) The server never saw it.
    assert_eq!(
        server.control.accepted_calls(),
        0,
        "a request whose outbound participant refused never reaches the server"
    );
    // The registry forgets it on its own, with the connection still open.
    tokio::time::timeout(Duration::from_mins(1), async {
        while runtime.outstanding_http_requests() != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("anti-hang guard: the refused request forgets its own lifecycle state");

    // (9) The connection is still alive and only the cancelled request id was
    // refused: a later independent call over the same generation succeeds.
    let healthy = tokio::time::timeout(
        Duration::from_mins(1),
        direct_executor_call(&fixture, &executor, &echo, 1),
    )
    .await
    .expect("anti-hang guard: an answered HTTP call settles");
    assert!(
        matches!(healthy.status, ToolExecutionStatus::Success),
        "the transport survives a refused request: {:?}",
        healthy.status
    );
    assert_eq!(
        server.control.accepted_calls(),
        1,
        "exactly the later healthy call reached the server"
    );

    runtime.close().await.expect("physical settlement");
    server.shutdown().await;
}

// ---------------------------------------------------------------------------
// MCP-01 (Issue #240): the SDK response-cache policy is a property of every
// connection generation, including one established by bounded reconnection
// ---------------------------------------------------------------------------

/// Issue #240: a **replacement** connection generation has the SDK response
/// cache disabled too, because the policy belongs to the one
/// connection-construction seam every generation is born from rather than to
/// a recovery branch.
///
/// ```text
/// generation 1 -- call echo --> received, then the server dies
///              <- OutcomeUnknown
/// generation 2   established by the bounded reconnect
///              -- tools/list -->  request #1 reached the server
///              -- tools/list -->  request #2 reached the server
/// ```
///
/// Every generation of this fixture declares a ten-minute SEP-2549 `ttlMs`
/// on its catalog, so an SDK response cache on the replacement peer would
/// answer the second refresh itself and the server would see one request,
/// not two. The proof is therefore behavioral and observed in the *server
/// process*: the fixture journals one `list:<generation>` line per
/// `tools/list` it actually answered, before answering it.
///
/// Synchronization: generation 1 journals the accepted call before it exits,
/// and the replacement is established by the ordinary pre-frontier
/// `McpConnection::acquire` of the second dispatch — no timing is involved
/// in either step.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_replacement_connection_generation_keeps_the_sdk_response_cache_disabled() {
    if recovery::serve_if_recovery_fixture_mode().await {
        return;
    }
    let fixture = common::native_fixture_without_extensions();
    let control = recovery::RecoveryControl::new(fixture.dir().path());
    let capability = recovery_capability(
        &fixture.runtime,
        "boundary_suites::mcp_recovery::a_replacement_connection_generation_keeps_the_sdk_response_cache_disabled",
        &control,
        &recovery::RecoveryScript {
            die_generations: vec![1],
            list_ttl_ms: Some(600_000),
            ..recovery::RecoveryScript::default()
        },
    )
    .await;
    let server_id = capability.server_id.clone();
    assert_eq!(
        control.catalog_requests(1),
        1,
        "capability preparation listed the first generation's catalog once"
    );

    // Generation 1 dies with the request in flight; the next dispatch
    // establishes the replacement through the ordinary bounded reconnect.
    let snapshot = capability.coordinator.current_snapshot();
    let ambiguous = direct_mcp_call(&fixture, &snapshot, recovery::TOOL_ECHO).await;
    assert!(
        matches!(ambiguous.status, ToolExecutionStatus::OutcomeUnknown { .. }),
        "the generation that died leaves its call's external outcome unknown: {:?}",
        ambiguous.status
    );
    wait_for_journal_entry(&control, recovery::JOURNAL_DIED);

    let served = direct_mcp_call(&fixture, &snapshot, recovery::TOOL_ECHO).await;
    assert!(
        matches!(served.status, ToolExecutionStatus::Success),
        "the replacement generation serves the next dispatch: {:?}",
        served.status
    );
    assert!(
        served
            .model_facing_projection()
            .as_text()
            .contains("generation 2"),
        "the replacement transport actually served it: {served:?}"
    );
    assert_eq!(
        control.established_generations(),
        2,
        "exactly one bounded replacement transport was established"
    );

    let replacement = capability
        .coordinator
        .current_mcp_runtime(&server_id)
        .expect("the connection published the replacement generation");
    assert_eq!(
        control.catalog_requests(2),
        0,
        "reconnection itself lists no catalog: the refreshes below are the only ones"
    );
    let first = replacement
        .list_tools()
        .await
        .expect("the replacement generation serves its catalog");
    assert_eq!(
        control.catalog_requests(2),
        1,
        "the first refresh reached the replacement generation"
    );
    let second = replacement
        .list_tools()
        .await
        .expect("the replacement generation serves its catalog again");
    assert_eq!(
        control.catalog_requests(2),
        2,
        "the replacement generation refetched despite the positive ttlMs: no SDK \
         response cache survived the reconnection"
    );
    assert_eq!(first, second, "the peer answered both refreshes itself");
    drop(capability);
}
