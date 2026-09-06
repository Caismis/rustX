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

use std::path::{Path, PathBuf};
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
use rustx::tools::mcp::fixture::recovery;
use rustx::tools::types::ToolExecutionStatus;
use support::fake::{FakeModel, FakeStep, ScriptedCall, fake_model, tool_call_events};
use tokio::sync::watch;

// ---------------------------------------------------------------------------
// Fixture wiring
// ---------------------------------------------------------------------------

/// The cross-process control files of one recovery-fixture server identity.
struct RecoveryPaths {
    journal: PathBuf,
    generation_file: PathBuf,
    release_dir: PathBuf,
}

impl RecoveryPaths {
    fn new(root: &Path) -> Self {
        let release_dir = root.join("release");
        std::fs::create_dir_all(&release_dir).expect("release directory");
        Self {
            journal: root.join("recovery.journal"),
            generation_file: root.join("recovery.generation"),
            release_dir,
        }
    }

    fn accepted_calls(&self, tool: &str) -> usize {
        recovery::accepted_calls(&self.journal, tool)
    }

    fn entries(&self) -> Vec<String> {
        recovery::journal_entries(&self.journal)
    }

    fn established_generations(&self) -> usize {
        self.entries()
            .into_iter()
            .filter(|line| line.starts_with(recovery::JOURNAL_GENERATION_PREFIX))
            .count()
    }
}

/// The binding of one recovery-fixture server: this test binary re-executed
/// as exactly `test_name` in recovery-fixture mode.
fn recovery_binding(
    test_name: &str,
    paths: &RecoveryPaths,
    die_generations: &[u64],
    refuse_generations: &[u64],
    corrupt_generations: &[u64],
    extra_tool_generations: &[u64],
    hang_pulses: u32,
) -> rustx::tools::mcp::McpServerBinding {
    rustx::tools::mcp::McpServerBinding {
        transport: rustx::tools::mcp::McpTransportConfig::Stdio {
            program: std::env::current_exe()
                .expect("test executable")
                .display()
                .to_string(),
            args: rustx::tools::mcp::fixture::fixture_spawn_args(test_name),
            cwd: None,
            environment: recovery::recovery_environment(
                &paths.journal,
                &paths.generation_file,
                &paths.release_dir,
                die_generations,
                refuse_generations,
                corrupt_generations,
                extra_tool_generations,
                hang_pulses,
            ),
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
    paths: &RecoveryPaths,
    die_generations: &[u64],
    refuse_generations: &[u64],
    corrupt_generations: &[u64],
    extra_tool_generations: &[u64],
    hang_pulses: u32,
) -> McpCapability {
    let dir = tempfile::tempdir().expect("capability temp dir");
    let server_id = McpServerId::new("recovery");
    let coordinator = CapabilityCoordinator::new(CapabilityCoordinatorConfig {
        conversation_id: tool_runtime.conversation_id().clone(),
        workspace: tool_runtime.workspace().clone(),
        base_tool_registry: Arc::new(rustx::tools::executor::ToolRegistry::new()),
        tool_activation: rustx::capabilities::ToolActivationPolicy::default(),
        skill_discovery: rustx::skills::SkillDiscoveryConfig::default(),
        mcp_servers: std::collections::BTreeMap::from([(
            server_id.clone(),
            recovery_binding(
                test_name,
                paths,
                die_generations,
                refuse_generations,
                corrupt_generations,
                extra_tool_generations,
                hang_pulses,
            ),
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

/// Observes the durable execution facts a controller needs: the start fact
/// and every forwarded MCP progress report.
struct ExecutionSignals {
    started: watch::Sender<bool>,
    progress: watch::Sender<u32>,
}

impl AgentExecutionObserver for ExecutionSignals {
    fn observe_event(&self, _attempt_id: &AttemptId, event: &RuntimeEvent) {
        if matches!(event, RuntimeEvent::ToolExecutionStarted { .. }) {
            self.started.send_replace(true);
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

    /// Waits for the in-band dispatch proof: the fixture's progress
    /// notification for the accepted `tools/call`.
    async fn wait_dispatched(&mut self) {
        self.progress
            .wait_for(|count| *count >= 1)
            .await
            .expect("the progress observation channel stays open");
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
/// contract under test, where a later ToolCall of the same conversation is
/// served by a replacement connection generation.
async fn run_mcp_calls<C, F>(
    fixture: &common::NativeFixture,
    capability: rustx::capabilities::AttemptCapabilityLease,
    attempt: &str,
    tool_names: &[&'static str],
    policy: ToolExecutionDeadlinePolicy,
    controller: C,
) -> common::DurableExecutionAudit
where
    C: FnOnce(Controls) -> F + Send + 'static,
    F: std::future::Future<Output = ()> + Send + 'static,
{
    let definitions = capability.snapshot().tool_registry().definitions();
    let mut turns = Vec::new();
    for (index, tool_name) in tool_names.iter().enumerate() {
        let tool_id = definitions
            .iter()
            .find(|definition| definition.name == *tool_name)
            .expect("the published catalog carries the tool")
            .id
            .as_str()
            .to_owned();
        let scripted = ScriptedCall {
            id: Box::leak(format!("call-{attempt}-{index}").into_boxed_str()),
            tool_id: Box::leak(tool_id.into_boxed_str()),
            name: tool_name,
            arguments: serde_json::json!({}),
        };
        let mut turn = vec![FakeStep::Emit(ModelEvent::Started)];
        for event in tool_call_events(0, &scripted) {
            turn.push(FakeStep::Emit(event));
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
        rustx::context::AgentStatusEngine::default(),
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
            attempt_id: AttemptId::new(&format!("attempt-205-{attempt}")),
            conversation: rustx::conversation::ConversationState::from_messages(vec![
                MessageBlock::User(UserMessageBlock {
                    id: MessageId::new(&format!("msg-user-205-{attempt}")),
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

/// Runs one attempt with exactly one MCP tool call.
async fn run_mcp_call<C, F>(
    fixture: &common::NativeFixture,
    capability: rustx::capabilities::AttemptCapabilityLease,
    attempt: &str,
    tool_name: &'static str,
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
        &audit.result.outcome
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
            call_id: rustx::runtime::identity::ToolCallId::new("direct-call"),
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

/// Waits for one journal entry, with a wall-clock anti-hang guard.
///
/// The entry itself is the ordering proof: the fixture writes it only after
/// the corresponding protocol event actually happened in the server process.
/// The guard only bounds how long the parent is willing to wait for a fact
/// that must eventually appear.
fn wait_for_journal_entry(paths: &RecoveryPaths, entry: &str) {
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    loop {
        if paths.entries().iter().any(|line| line == entry) {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "anti-hang guard: the fixture never journaled {entry:?}; journal: {:?}",
            paths.entries()
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
    let fixture = common::native_fixture();
    let paths = RecoveryPaths::new(fixture.dir().path());
    let capability = recovery_capability(
        &fixture.runtime,
        "boundary_suites::mcp_recovery::an_unanswered_mcp_call_is_bounded_by_the_generic_hard_deadline",
        &paths,
        &[],
        &[],
        &[],
        &[],
        0,
    )
    .await;
    let lease = capability.coordinator.acquire_attempt_lease();
    let audit = run_mcp_call(
        &fixture,
        lease,
        "hang",
        recovery::TOOL_HANG,
        ToolExecutionDeadlinePolicy {
            hard_deadline: Duration::from_secs(5),
            idle_liveness: None,
        },
        |mut controls| async move {
            controls.wait_started().await;
            // The in-band gate: the request provably reached the server.
            controls.wait_dispatched().await;
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
    wait_for_journal_entry(&paths, &format!("{}1", recovery::JOURNAL_CANCELLED_PREFIX));
    assert_eq!(
        paths.accepted_calls(recovery::TOOL_HANG),
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
    let fixture = common::native_fixture();
    let paths = RecoveryPaths::new(fixture.dir().path());
    let capability = recovery_capability(
        &fixture.runtime,
        "boundary_suites::mcp_recovery::mcp_progress_refreshes_idle_liveness_and_never_extends_the_hard_deadline",
        &paths,
        &[],
        &[],
        &[],
        &[],
        3,
    )
    .await;
    let lease = capability.coordinator.acquire_attempt_lease();
    let release_dir = paths.release_dir.clone();
    let audit = run_mcp_call(
        &fixture,
        lease,
        "hang",
        recovery::TOOL_HANG,
        ToolExecutionDeadlinePolicy {
            hard_deadline: Duration::from_millis(3_000),
            idle_liveness: Some(Duration::from_millis(1_000)),
        },
        move |mut controls| async move {
            controls.wait_started().await;
            controls.wait_dispatched().await;
            for pulse in 1..=3_u32 {
                // 900 < 1000: the current idle window survives this advance.
                controls.clock.advance(900);
                recovery::release_hang_pulse(&release_dir, pulse);
                // The refreshed window is published before this returns.
                controls.wait_progress_at_least(pulse + 1).await;
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
        progress_facts, 4,
        "one dispatch notification plus three released pulses were forwarded as \
         durable liveness evidence"
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
    let fixture = common::native_fixture();
    let paths = RecoveryPaths::new(fixture.dir().path());
    // Only generation 1 dies with the request in flight; generation 2 serves
    // normally.
    let capability = recovery_capability(
        &fixture.runtime,
        "boundary_suites::mcp_recovery::transport_loss_after_dispatch_is_unknown_and_reconnect_never_replays_it",
        &paths,
        &[1],
        &[],
        &[],
        &[],
        0,
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
            hard_deadline: Duration::from_secs(60),
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
        &audit.result.outcome
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
    wait_for_journal_entry(&paths, recovery::JOURNAL_DIED);
    assert_eq!(
        paths.established_generations(),
        2,
        "exactly one bounded replacement transport was established"
    );
    assert_eq!(
        paths.accepted_calls(recovery::TOOL_ECHO),
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
    let fixture = common::native_fixture();
    let paths = RecoveryPaths::new(fixture.dir().path());
    let capability = recovery_capability(
        &fixture.runtime,
        "boundary_suites::mcp_recovery::a_failed_reconnect_is_a_pre_frontier_failure_and_never_replays_the_ambiguous_call",
        &paths,
        &[1],
        &[2],
        &[],
        &[],
        0,
    )
    .await;

    let audit = run_mcp_calls(
        &fixture,
        capability.coordinator.acquire_attempt_lease(),
        "reconnect",
        &[recovery::TOOL_ECHO, recovery::TOOL_ECHO],
        ToolExecutionDeadlinePolicy {
            hard_deadline: Duration::from_secs(60),
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
        &audit.result.outcome
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

    wait_for_journal_entry(&paths, recovery::JOURNAL_DIED);
    wait_for_journal_entry(&paths, &format!("{}2", recovery::JOURNAL_REFUSED_PREFIX));
    assert_eq!(
        paths.established_generations(),
        2,
        "exactly one bounded replacement attempt was made, not a reconnect loop"
    );
    assert_eq!(
        paths.accepted_calls(recovery::TOOL_ECHO),
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
    let fixture = common::native_fixture();
    let paths = RecoveryPaths::new(fixture.dir().path());
    let capability = recovery_capability(
        &fixture.runtime,
        "boundary_suites::mcp_recovery::cancellation_without_a_remote_response_is_exactly_one_outcome_unknown",
        &paths,
        &[],
        &[],
        &[],
        &[],
        0,
    )
    .await;
    let lease = capability.coordinator.acquire_attempt_lease();
    let audit = run_mcp_call(
        &fixture,
        lease,
        "cancel",
        recovery::TOOL_HANG,
        ToolExecutionDeadlinePolicy {
            hard_deadline: Duration::from_secs(60),
            idle_liveness: None,
        },
        move |mut controls| async move {
            controls.wait_started().await;
            controls.wait_dispatched().await;
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
    wait_for_journal_entry(&paths, &format!("{}1", recovery::JOURNAL_CANCELLED_PREFIX));
    assert_eq!(
        paths.accepted_calls(recovery::TOOL_HANG),
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
    let fixture = common::native_fixture();
    let paths = RecoveryPaths::new(fixture.dir().path());
    let capability = recovery_capability(
        &fixture.runtime,
        "boundary_suites::mcp_recovery::a_remote_response_that_won_arbitration_survives_a_later_cancellation",
        &paths,
        &[],
        &[],
        &[],
        &[],
        0,
    )
    .await;
    let lease = capability.coordinator.acquire_attempt_lease();
    let audit = run_mcp_call(
        &fixture,
        lease,
        "won",
        recovery::TOOL_ECHO,
        ToolExecutionDeadlinePolicy {
            hard_deadline: Duration::from_secs(60),
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
    assert_eq!(paths.accepted_calls(recovery::TOOL_ECHO), 1);
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
    let fixture = common::native_fixture();
    let paths = RecoveryPaths::new(fixture.dir().path());
    let capability = recovery_capability(
        &fixture.runtime,
        "boundary_suites::mcp_recovery::drain_closes_every_connection_generation_and_refuses_reconnection",
        &paths,
        &[1],
        &[],
        &[],
        &[],
        0,
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
            hard_deadline: Duration::from_secs(60),
            idle_liveness: None,
        },
        |_controls| async move {},
    )
    .await;
    let results = tool_results(&audit);
    assert_eq!(results.len(), 2);
    assert!(matches!(results[1].status, ToolExecutionStatus::Success));
    assert_eq!(paths.established_generations(), 2);

    // The published capability generation is captured before drain, exactly
    // as a still-running consumer would hold it.
    let published = capability.coordinator.current_snapshot();
    capability
        .coordinator
        .drain_conversation_owned()
        .await
        .expect("every established transport generation proves settlement");
    let generations_after_drain = paths.established_generations();

    let result = direct_mcp_call(&fixture, &published, recovery::TOOL_ECHO).await;
    assert!(
        matches!(result.status, ToolExecutionStatus::Failed { .. }),
        "a drained MCP plane settles later calls as ordinary pre-frontier failures, \
         never as a second terminal outcome: {:?}",
        result.status
    );
    assert_eq!(
        paths.established_generations(),
        generations_after_drain,
        "drain closes publication authority: no reconnection may spawn a server after it"
    );
    assert_eq!(
        paths.accepted_calls(recovery::TOOL_ECHO),
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
    let fixture = common::native_fixture();
    let paths = RecoveryPaths::new(fixture.dir().path());
    // Generation 2 — the one a refresh would establish — refuses to
    // handshake, so the refresh cannot produce a validated generation.
    let capability = recovery_capability(
        &fixture.runtime,
        "boundary_suites::mcp_recovery::a_failed_capability_refresh_keeps_the_last_known_good_generation",
        &paths,
        &[],
        &[2],
        &[],
        &[],
        0,
    )
    .await;
    let published = capability.definition_names();
    assert_eq!(
        published,
        vec![
            recovery::TOOL_ECHO.to_owned(),
            recovery::TOOL_HANG.to_owned()
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

    wait_for_journal_entry(&paths, &format!("{}2", recovery::JOURNAL_REFUSED_PREFIX));
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
            hard_deadline: Duration::from_secs(60),
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
    let fixture = common::native_fixture();
    let paths = RecoveryPaths::new(fixture.dir().path());
    let capability = recovery_capability(
        &fixture.runtime,
        "boundary_suites::mcp_recovery::a_successful_refresh_atomically_replaces_the_previous_generation",
        &paths,
        &[],
        &[],
        &[],
        &[2],
        0,
    )
    .await;
    let before = capability.coordinator.current_snapshot();
    assert_eq!(
        capability.definition_names(),
        vec![
            recovery::TOOL_ECHO.to_owned(),
            recovery::TOOL_HANG.to_owned()
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
            recovery::TOOL_ECHO.to_owned(),
            recovery::TOOL_HANG.to_owned()
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
            recovery::TOOL_ECHO.to_owned(),
            recovery::TOOL_HANG.to_owned()
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
    let fixture = common::native_fixture();
    let paths = RecoveryPaths::new(fixture.dir().path());
    let capability = recovery_capability(
        &fixture.runtime,
        "boundary_suites::mcp_recovery::tool_admission_never_observes_a_candidate_under_construction",
        &paths,
        &[],
        &[],
        &[],
        &[2],
        0,
    )
    .await;
    let authoritative = vec![
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
    let fixture = common::native_fixture();
    let paths = RecoveryPaths::new(fixture.dir().path());
    let capability = recovery_capability(
        &fixture.runtime,
        "boundary_suites::mcp_recovery::a_poisoned_generation_fails_closed_and_is_replaced_without_replay",
        &paths,
        &[],
        &[],
        &[1],
        &[],
        0,
    )
    .await;

    let audit = run_mcp_calls(
        &fixture,
        capability.coordinator.acquire_attempt_lease(),
        "poison",
        &[recovery::TOOL_ECHO, recovery::TOOL_ECHO],
        ToolExecutionDeadlinePolicy {
            hard_deadline: Duration::from_secs(60),
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
        &audit.result.outcome
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

    wait_for_journal_entry(&paths, recovery::JOURNAL_CORRUPTED);
    assert_eq!(
        paths.established_generations(),
        2,
        "the poisoned generation is retired and replaced exactly once"
    );
    assert_eq!(
        paths.accepted_calls(recovery::TOOL_ECHO),
        2,
        "the poisoned invocation was never resubmitted to the replacement"
    );
    drop(capability);
}
