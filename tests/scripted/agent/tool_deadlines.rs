//! Issue #204: deterministic generic foreground tool execution-liveness
//! deadline regressions.
//!
//! The Agent Loop owns the one generic deadline lifecycle: a frozen
//! per-admission [`ToolExecutionDeadlinePolicy`], a hard lifetime deadline
//! that progress never extends, and an idle-liveness deadline that each
//! honest progress observation refreshes. Deadline expiration is
//! cancellation/liveness *intent*, never proof of settlement: the loop
//! requests physical cancellation of exactly the admitted execution and
//! awaits the executor's settlement, committing `TimedOut` only when the
//! executor proved terminality because of the deadline, keeping an
//! executor-proven completion or failure that won the physical race, and
//! keeping an honest `OutcomeUnknown` when a post-frontier termination
//! cannot be proven (the Issue #202 outcome-certainty contract).
//!
//! Every cut below is driven by the manual monotonic clock and explicit
//! watch-channel gates; wall-clock time appears only as an outer anti-hang
//! guard. The user-cancellation-vs-completion race matrix of the same
//! arbitration point is owned by the Issue #136 suite
//! (`tool_cancellation.rs`); this suite covers the deadline winner and its
//! races.

use super::super::{common, support};

use std::sync::Arc;
use std::time::Duration;

use futures_util::future::BoxFuture;
use rustx::agent::{AgentCancellation, AgentExecution, AgentExecutionRequest};
use rustx::events::types::{AttemptOutcome, RuntimeEvent};
use rustx::message::content::TextBlock;
use rustx::message::types::{
    ContentBlockIndex, MessageBlock, ToolMessageBlock, UserContentBlock, UserMessageBlock,
    UserSource,
};
use rustx::model::event::ModelEvent;
use rustx::model::finish::ModelFinishReason;
use rustx::runtime::identity::{AgentId, AttemptId, ConversationId, MessageId, ToolCallId};
use rustx::runtime::types::CancellationReason;
use rustx::runtime::{ManualMonotonicClock, MonotonicClock};
use rustx::tools::deadline::{ToolDeadlineKind, ToolExecutionDeadlinePolicy};
use rustx::tools::executor::{ToolExecutionContext, ToolExecutor, ToolRegistry};
use rustx::tools::types::{
    ToolCancellationPhase, ToolConcurrencyPolicy, ToolExecutionPolicy, ToolExecutionResult,
    ToolExecutionStatus, ToolInvocation, ToolProgress,
};
use support::fake::{
    FakeModel, FakeStep, FakeTool, ScriptedCall, await_started, fake_model, success_result,
    tool_call_events,
};
use tokio::sync::watch;

const CONVERSATION: &str = "conv-204";
const GUARD: Duration = Duration::from_secs(5);

fn request(model: &Arc<FakeModel>) -> AgentExecutionRequest {
    AgentExecutionRequest {
        agent_id: AgentId::new("agent-204"),
        conversation_id: ConversationId::new(CONVERSATION),
        attempt_id: AttemptId::new("attempt-204"),
        conversation: rustx::conversation::ConversationState::from_messages(vec![
            MessageBlock::User(UserMessageBlock {
                id: MessageId::new("msg-user-204"),
                content: vec![UserContentBlock::Text(TextBlock {
                    text: "Run the tool".to_owned(),
                })],
                source: UserSource::Human,
                kind: rustx::message::types::InboundKind::Message,
                timestamp: None,
            }),
        ])
        .expect("valid fixture conversation"),
        initial_turn_trigger: rustx::runtime::inbound::InitialTurnTrigger::Continuation,
        model: support::attempt_model(model.clone(), "fake-model"),
    }
}

fn context_runtime(
    model: &Arc<FakeModel>,
    clock: Arc<ManualMonotonicClock>,
) -> rustx::context::ContextRuntime {
    let snapshot = support::attempt_model(model.clone(), "fake-model");
    rustx::context::ContextRuntime::for_attempt(
        rustx::context::SessionContextPolicy {
            reserve_tokens: 0,
            keep_recent_tokens: 0,
            summary_output_cap: None,
        },
        Arc::new(rustx::context::DefaultTokenEstimator),
        rustx::context::AgentStatusEngine::default(),
        &snapshot,
        rustx::model::ModelTimeoutPolicy::default(),
        clock,
    )
    .expect("valid context runtime")
}

fn execution_policy(
    tool_deadline_policy: ToolExecutionDeadlinePolicy,
    clock: Arc<ManualMonotonicClock>,
) -> crate::agent::execution::AgentExecutionRuntimePolicy {
    crate::agent::execution::AgentExecutionRuntimePolicy {
        model_timeout_policy: rustx::model::ModelTimeoutPolicy::default(),
        tool_deadline_policy,
        monotonic_clock: clock as Arc<dyn MonotonicClock>,
        subagent_context: None,
        workflow_output: None,
    }
}

fn deadline_policy(hard_ms: u64, idle_ms: Option<u64>) -> ToolExecutionDeadlinePolicy {
    ToolExecutionDeadlinePolicy {
        hard_deadline: Duration::from_millis(hard_ms),
        idle_liveness: idle_ms.map(Duration::from_millis),
    }
}

fn call(id: &'static str, tool_id: &'static str, name: &'static str) -> ScriptedCall {
    ScriptedCall {
        id,
        tool_id,
        name,
        arguments: serde_json::json!({}),
    }
}

/// A tool-call turn scripted from `calls`, followed by a plain Stop turn.
fn tool_turn_then_stop(calls: &[ScriptedCall]) -> Vec<Vec<FakeStep>> {
    let mut first = vec![FakeStep::Emit(ModelEvent::Started)];
    for (index, scripted) in calls.iter().enumerate() {
        for event in tool_call_events(
            u32::try_from(index).expect("the scripted batch is small"),
            scripted,
        ) {
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
                block_index: ContentBlockIndex::new(0),
                text: "done".to_owned(),
            }),
            FakeStep::Emit(ModelEvent::Completed {
                finish_reason: ModelFinishReason::Stop,
                usage: None,
            }),
        ],
    ]
}

/// What a [`DeadlineProbeTool`] settles with when the invocation's
/// cancellation signal wins over a release gate.
#[derive(Clone, Copy)]
enum ProbeCancelSettlement {
    /// The executor proves physical cancellation (a stopped local
    /// operation); the loop normalizes this to `TimedOut` when a deadline
    /// won the arbitration.
    Cancelled,
    /// The call crossed its external-effect frontier and the executor
    /// cannot prove remote terminality.
    OutcomeUnknown,
    /// The executor proves its normal completion won the physical race even
    /// though cancellation was requested.
    Completed,
}

/// A deterministic scripted executor for the deadline lifecycle: it parks
/// on one release gate per phase, reports the phase's progress message
/// after the gate releases, and settles with `result` when the final gate
/// releases. Every park races the invocation's cancellation signal, whose
/// observation is reported through `cancel_observed` before the programmed
/// [`ProbeCancelSettlement`] is returned. An optional settle gate blocks
/// the *return* of the settled outcome, modelling an executor whose
/// physical settlement evidence arrives late.
struct DeadlineProbeTool {
    result: ToolExecutionResult,
    phase_messages: Vec<String>,
    gates: Vec<watch::Sender<bool>>,
    on_cancel: ProbeCancelSettlement,
    settle_gate: Option<watch::Sender<bool>>,
    started: watch::Sender<bool>,
    reported: watch::Sender<u32>,
    cancel_observed: watch::Sender<bool>,
}

/// The controller-side handles of a registered [`DeadlineProbeTool`].
struct ProbeHandles {
    started: watch::Receiver<bool>,
    reported: watch::Receiver<u32>,
    cancel_observed: watch::Receiver<bool>,
    gates: Vec<watch::Sender<bool>>,
    settle_gate: Option<watch::Sender<bool>>,
}

impl DeadlineProbeTool {
    fn cancel_settlement(&self) -> ToolExecutionResult {
        self.cancel_observed.send_replace(true);
        match self.on_cancel {
            ProbeCancelSettlement::Cancelled => ToolExecutionResult {
                status: ToolExecutionStatus::Cancelled {
                    reason: CancellationReason::UserRequested,
                    phase: ToolCancellationPhase::DuringExecution,
                },
                content: Vec::new(),
                duration_ms: 0,
                exit_code: None,
                artifacts: Vec::new(),
                truncation: None,
                managed_output: None,
            },
            ProbeCancelSettlement::OutcomeUnknown => ToolExecutionResult {
                status: ToolExecutionStatus::OutcomeUnknown {
                    detail: "request crossed the external-effect frontier; remote termination \
                             could not be confirmed"
                        .to_owned(),
                },
                content: Vec::new(),
                duration_ms: 0,
                exit_code: None,
                artifacts: Vec::new(),
                truncation: None,
                managed_output: None,
            },
            ProbeCancelSettlement::Completed => self.result.clone(),
        }
    }
}

impl ToolExecutor for DeadlineProbeTool {
    fn execute<'a>(
        &'a self,
        _invocation: ToolInvocation,
        context: ToolExecutionContext<'a>,
    ) -> BoxFuture<'a, ToolExecutionResult> {
        Box::pin(async move {
            self.started.send_replace(true);
            let mut cancelled = None;
            for (index, gate) in self.gates.iter().enumerate() {
                let mut released = gate.subscribe();
                tokio::select! {
                    biased;
                    () = context.cancellation.cancelled() => {
                        cancelled = Some(self.cancel_settlement());
                        break;
                    }
                    ok = released.wait_for(|is_released| *is_released) => {
                        ok.expect("probe release gate stays open");
                        if let Some(message) = self.phase_messages.get(index) {
                            context.progress.report(ToolProgress {
                                message: Some(message.clone()),
                                completed: None,
                                total: None,
                            });
                            self.reported.send_modify(|count| *count += 1);
                        }
                    }
                }
            }
            let outcome = cancelled.unwrap_or_else(|| self.result.clone());
            // The settlement gate models late physical settlement evidence:
            // the outcome is already decided, but the execution future does
            // not resolve until the executor's settlement is real.
            if let Some(settle_gate) = &self.settle_gate {
                let mut released = settle_gate.subscribe();
                released
                    .wait_for(|is_released| *is_released)
                    .await
                    .expect("probe settle gate stays open");
            }
            outcome
        })
    }
}

/// Registers a probe tool and returns its controller handles. The probe has
/// `phase_messages.len() + 1` release gates: gate `i` releases phase `i`
/// (reporting its progress message), and the final gate settles the call
/// with `result`.
#[allow(clippy::too_many_arguments)] // one scripted-fixture constructor
fn register_probe(
    registry: &mut ToolRegistry,
    name: &str,
    tool_id: &str,
    concurrency: ToolConcurrencyPolicy,
    result: ToolExecutionResult,
    phase_messages: &[&str],
    on_cancel: ProbeCancelSettlement,
    with_settle_gate: bool,
) -> ProbeHandles {
    let gates: Vec<watch::Sender<bool>> = (0..=phase_messages.len())
        .map(|_| watch::channel(false).0)
        .collect();
    let settle_gate = with_settle_gate.then(|| watch::channel(false).0);
    let probe = DeadlineProbeTool {
        result,
        phase_messages: phase_messages
            .iter()
            .map(|message| (*message).to_owned())
            .collect(),
        gates: gates.clone(),
        on_cancel,
        settle_gate: settle_gate.clone(),
        started: watch::Sender::new(false),
        reported: watch::Sender::new(0),
        cancel_observed: watch::Sender::new(false),
    };
    let handles = ProbeHandles {
        started: probe.started.subscribe(),
        reported: probe.reported.subscribe(),
        cancel_observed: probe.cancel_observed.subscribe(),
        gates,
        settle_gate,
    };
    registry
        .register(
            common::tool_policies(
                name,
                tool_id,
                ToolExecutionPolicy::ForegroundOnly,
                concurrency,
            ),
            Arc::new(probe),
        )
        .expect("probe tool registration");
    handles
}

async fn run(
    model: &Arc<FakeModel>,
    tools: ToolRegistry,
    policy: ToolExecutionDeadlinePolicy,
    clock: Arc<ManualMonotonicClock>,
    cancellation: &AgentCancellation,
) -> common::DurableExecutionAudit {
    let tool_runtime = common::tool_runtime(CONVERSATION);
    let store = tool_runtime.durable_store();
    let capability = common::capability_lease(tools, &tool_runtime).await;
    let execution = AgentExecution::new(
        request(model),
        capability.into_lease(),
        cancellation,
        execution_policy(policy, clock.clone()),
        context_runtime(model, clock),
        &tool_runtime,
        rustx::agent::AttemptLifecycle::inert(),
    )
    .expect("conversation identity matches the tool runtime");
    let result = tokio::time::timeout(GUARD, execution.run())
        .await
        .expect("Issue #204 execution must settle without wall-clock waiting");
    common::durable_agent_result(result, store.as_ref())
}

fn tool_messages(audit: &common::DurableExecutionAudit) -> Vec<&ToolMessageBlock> {
    audit
        .result
        .messages()
        .iter()
        .filter_map(|message| match message {
            MessageBlock::Tool(tool) => Some(tool),
            _ => None,
        })
        .collect()
}

/// The durable execution-fact sequence of one call, in journal order.
fn fact_sequence(audit: &common::DurableExecutionAudit, call_id: &str) -> Vec<String> {
    audit
        .event_history
        .iter()
        .filter_map(|event| match event {
            RuntimeEvent::ToolExecutionStarted { tool_call_id, .. }
                if tool_call_id.as_str() == call_id =>
            {
                Some("started".to_owned())
            }
            RuntimeEvent::ToolExecutionProgress { tool_call_id, .. }
                if tool_call_id.as_str() == call_id =>
            {
                Some("progress".to_owned())
            }
            RuntimeEvent::ToolExecutionDeadlineFired {
                tool_call_id, kind, ..
            } if tool_call_id.as_str() == call_id => Some(match kind {
                ToolDeadlineKind::Hard => "deadline:hard".to_owned(),
                ToolDeadlineKind::Idle => "deadline:idle".to_owned(),
            }),
            RuntimeEvent::ToolExecutionCompleted { tool_call_id, .. }
                if tool_call_id.as_str() == call_id =>
            {
                Some("completed".to_owned())
            }
            _ => None,
        })
        .collect()
}

fn deadline_kinds(audit: &common::DurableExecutionAudit, call_id: &str) -> Vec<ToolDeadlineKind> {
    audit
        .event_history
        .iter()
        .filter_map(|event| match event {
            RuntimeEvent::ToolExecutionDeadlineFired {
                tool_call_id, kind, ..
            } if tool_call_id.as_str() == call_id => Some(*kind),
            _ => None,
        })
        .collect()
}

/// A: an executor that starts and never completes is bounded by the hard
/// deadline. The deadline fires cancellation intent, the executor proves
/// physical cancellation, and the one canonical result is `TimedOut` —
/// committed exactly once, after the intent fact.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue204_hard_deadline_bounds_a_hung_executor() {
    let scripted = call("call-hung", "tool-hung", "hung");
    let model = fake_model(tool_turn_then_stop(&[scripted]));
    let (tool, _release) = FakeTool::parking(
        common::tool("hung", "tool-hung"),
        success_result("not reached"),
    );
    let mut started = tool.started();
    let calls = tool.calls();
    let mut tools = ToolRegistry::new();
    tool.register(&mut tools);

    let clock = Arc::new(ManualMonotonicClock::new());
    let controller_clock = clock.clone();
    let controller = tokio::spawn(async move {
        await_started(&mut started, "hung tool").await;
        controller_clock.advance(60_000);
    });
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let audit = run(
        &model,
        tools,
        deadline_policy(60_000, None),
        clock,
        &cancellation,
    )
    .await;
    controller.await.expect("deadline controller");

    let messages = tool_messages(&audit);
    assert_eq!(messages.len(), 1, "exactly one canonical tool result");
    assert!(
        matches!(messages[0].result.status, ToolExecutionStatus::TimedOut),
        "executor-proven cancellation after the hard deadline is the proven timeout: {:?}",
        messages[0].result.status
    );
    assert_eq!(calls.borrow().len(), 1, "one executor invocation");
    assert_eq!(
        fact_sequence(&audit, "call-hung"),
        vec!["started", "deadline:hard", "completed"],
        "intent fact lands between start and the terminal completion fact"
    );
    assert!(matches!(
        audit.result.outcome,
        AttemptOutcome::Completed {
            finish_reason: ModelFinishReason::Stop
        }
    ));
}

/// B: a progress observation refreshes the idle watchdog. The idle window
/// would first have expired at `t=10_000`; the report at `t=9_000` moves
/// the idle deadline to `t=19_000`, so the hard deadline at `t=15_000` is the
/// winner — an `Idle` winner here would prove the refresh was lost.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue204_progress_refreshes_idle_liveness() {
    let scripted = call("call-progress", "tool-progress", "progressive");
    let model = fake_model(tool_turn_then_stop(&[scripted]));
    let mut tools = ToolRegistry::new();
    let probe = register_probe(
        &mut tools,
        "progressive",
        "tool-progress",
        ToolConcurrencyPolicy::Sequential,
        success_result("not reached"),
        &["phase one"],
        ProbeCancelSettlement::Cancelled,
        false,
    );
    let mut started = probe.started;
    let mut reported = probe.reported;
    let gates = probe.gates;

    let clock = Arc::new(ManualMonotonicClock::new());
    let controller_clock = clock.clone();
    let controller = tokio::spawn(async move {
        await_started(&mut started, "progress tool").await;
        controller_clock.advance(9_000);
        gates[0].send_replace(true);
        reported
            .wait_for(|count| *count == 1)
            .await
            .expect("progress observation channel stays open");
        // t=15_000: past the unrefreshed idle window (10_000), before the
        // refreshed one (19_000). Only the hard deadline may fire.
        controller_clock.advance(6_000);
    });
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let audit = run(
        &model,
        tools,
        deadline_policy(15_000, Some(10_000)),
        clock,
        &cancellation,
    )
    .await;
    controller.await.expect("idle refresh controller");

    let messages = tool_messages(&audit);
    assert_eq!(messages.len(), 1);
    assert!(matches!(
        messages[0].result.status,
        ToolExecutionStatus::TimedOut
    ));
    assert_eq!(
        deadline_kinds(&audit, "call-progress"),
        vec![ToolDeadlineKind::Hard],
        "the refreshed idle watchdog must not fire at the stale window"
    );
    assert_eq!(
        fact_sequence(&audit, "call-progress"),
        vec!["started", "progress", "deadline:hard", "completed"]
    );
}

/// C: progress never extends the hard deadline. Reports at `t=9_000` and
/// `t=17_000` keep the idle watchdog alive forever, yet the hard deadline
/// still wins at exactly `t=25_000`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue204_progress_never_extends_the_hard_deadline() {
    let scripted = call("call-chatty", "tool-chatty", "chatty");
    let model = fake_model(tool_turn_then_stop(&[scripted]));
    let mut tools = ToolRegistry::new();
    let probe = register_probe(
        &mut tools,
        "chatty",
        "tool-chatty",
        ToolConcurrencyPolicy::Sequential,
        success_result("not reached"),
        &["phase one", "phase two"],
        ProbeCancelSettlement::Cancelled,
        false,
    );
    let mut started = probe.started;
    let mut reported = probe.reported;
    let gates = probe.gates;

    let clock = Arc::new(ManualMonotonicClock::new());
    let controller_clock = clock.clone();
    let controller = tokio::spawn(async move {
        await_started(&mut started, "chatty tool").await;
        controller_clock.advance(9_000);
        gates[0].send_replace(true);
        reported
            .wait_for(|count| *count == 1)
            .await
            .expect("first progress observation");
        controller_clock.advance(8_000);
        gates[1].send_replace(true);
        reported
            .wait_for(|count| *count == 2)
            .await
            .expect("second progress observation");
        // t=25_000: the idle window was refreshed to t=27_000 by the second
        // report, so only the hard deadline is eligible.
        controller_clock.advance(8_000);
    });
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let audit = run(
        &model,
        tools,
        deadline_policy(25_000, Some(10_000)),
        clock,
        &cancellation,
    )
    .await;
    controller.await.expect("chatty controller");

    let messages = tool_messages(&audit);
    assert_eq!(messages.len(), 1);
    assert!(
        matches!(messages[0].result.status, ToolExecutionStatus::TimedOut),
        "the immutable hard deadline wins despite continuous progress: {:?}",
        messages[0].result.status
    );
    assert_eq!(
        fact_sequence(&audit, "call-chatty"),
        vec![
            "started",
            "progress",
            "progress",
            "deadline:hard",
            "completed"
        ]
    );
}

/// D: an execution that produces no progress evidence is cancelled by the
/// idle-liveness deadline while the hard deadline remains far away.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue204_idle_deadline_cancels_a_silent_execution() {
    let scripted = call("call-silent", "tool-silent", "silent");
    let model = fake_model(tool_turn_then_stop(&[scripted]));
    let mut tools = ToolRegistry::new();
    let probe = register_probe(
        &mut tools,
        "silent",
        "tool-silent",
        ToolConcurrencyPolicy::Sequential,
        success_result("not reached"),
        &[],
        ProbeCancelSettlement::Cancelled,
        false,
    );
    let mut started = probe.started;

    let clock = Arc::new(ManualMonotonicClock::new());
    let controller_clock = clock.clone();
    let controller = tokio::spawn(async move {
        await_started(&mut started, "silent tool").await;
        controller_clock.advance(10_000);
    });
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let audit = run(
        &model,
        tools,
        deadline_policy(100_000, Some(10_000)),
        clock,
        &cancellation,
    )
    .await;
    controller.await.expect("idle controller");

    let messages = tool_messages(&audit);
    assert_eq!(messages.len(), 1);
    assert!(matches!(
        messages[0].result.status,
        ToolExecutionStatus::TimedOut
    ));
    assert_eq!(
        fact_sequence(&audit, "call-silent"),
        vec!["started", "deadline:idle", "completed"],
        "the idle deadline is the one cancellation cause"
    );
}

/// H: when the hard and idle deadlines become eligible at the same instant,
/// the documented arbitration order (hard before idle) produces exactly one
/// cancellation cause and exactly one terminal result — never two intents
/// and never a duplicate settlement.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue204_simultaneous_hard_and_idle_have_one_winner() {
    let scripted = call("call-tie", "tool-tie", "tied");
    let model = fake_model(tool_turn_then_stop(&[scripted]));
    let mut tools = ToolRegistry::new();
    let probe = register_probe(
        &mut tools,
        "tied",
        "tool-tie",
        ToolConcurrencyPolicy::Sequential,
        success_result("not reached"),
        &[],
        ProbeCancelSettlement::Cancelled,
        false,
    );
    let mut started = probe.started;

    let clock = Arc::new(ManualMonotonicClock::new());
    let controller_clock = clock.clone();
    let controller = tokio::spawn(async move {
        await_started(&mut started, "tied tool").await;
        controller_clock.advance(10_000);
    });
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let audit = run(
        &model,
        tools,
        deadline_policy(10_000, Some(10_000)),
        clock,
        &cancellation,
    )
    .await;
    controller.await.expect("tie controller");

    let messages = tool_messages(&audit);
    assert_eq!(messages.len(), 1, "no duplicate settlement");
    assert!(matches!(
        messages[0].result.status,
        ToolExecutionStatus::TimedOut
    ));
    assert_eq!(
        fact_sequence(&audit, "call-tie"),
        vec!["started", "deadline:hard", "completed"],
        "the hard deadline is the documented simultaneous-eligibility winner"
    );
}

/// F: an executor that crossed its external-effect frontier and cannot
/// prove remote terminality settles `OutcomeUnknown` after the deadline's
/// cancellation intent — never `TimedOut`, which requires proven
/// terminality under the Issue #202 contract.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue204_unconfirmed_external_termination_is_outcome_unknown() {
    let scripted = call("call-remote", "tool-remote", "remote");
    let model = fake_model(tool_turn_then_stop(&[scripted]));
    let mut tools = ToolRegistry::new();
    let probe = register_probe(
        &mut tools,
        "remote",
        "tool-remote",
        ToolConcurrencyPolicy::Sequential,
        success_result("not reached"),
        &[],
        ProbeCancelSettlement::OutcomeUnknown,
        false,
    );
    let mut started = probe.started;
    let mut cancel_observed = probe.cancel_observed;

    let clock = Arc::new(ManualMonotonicClock::new());
    let controller_clock = clock.clone();
    let controller = tokio::spawn(async move {
        await_started(&mut started, "remote tool").await;
        controller_clock.advance(20_000);
        // The deadline's cancellation intent provably reached the executor
        // before the run may settle.
        cancel_observed
            .wait_for(|observed| *observed)
            .await
            .expect("cancellation observation channel stays open");
    });
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let audit = run(
        &model,
        tools,
        deadline_policy(20_000, None),
        clock,
        &cancellation,
    )
    .await;
    controller.await.expect("outcome-unknown controller");

    let messages = tool_messages(&audit);
    assert_eq!(messages.len(), 1);
    let ToolExecutionStatus::OutcomeUnknown { detail } = &messages[0].result.status else {
        panic!(
            "post-frontier unconfirmed termination is never TimedOut: {:?}",
            messages[0].result.status
        );
    };
    assert!(detail.contains("could not be confirmed"));
    assert_eq!(
        fact_sequence(&audit, "call-remote"),
        vec!["started", "deadline:hard", "completed"],
        "the intent fact is journaled even when the outcome stays unknown"
    );
}

/// I (completion wins the physical race): the deadline's cancellation
/// intent fires, but the executor proves its normal completion settled
/// first. The proven completion is authoritative; the deadline intent is
/// journaled as an observational fact and nothing more.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue204_executor_proven_completion_survives_deadline_intent() {
    let scripted = call("call-race", "tool-race", "racing");
    let model = fake_model(tool_turn_then_stop(&[scripted]));
    let mut tools = ToolRegistry::new();
    let probe = register_probe(
        &mut tools,
        "racing",
        "tool-race",
        ToolConcurrencyPolicy::Sequential,
        success_result("real completion"),
        &[],
        ProbeCancelSettlement::Completed,
        false,
    );
    let mut started = probe.started;
    let mut cancel_observed = probe.cancel_observed;

    let clock = Arc::new(ManualMonotonicClock::new());
    let controller_clock = clock.clone();
    let controller = tokio::spawn(async move {
        await_started(&mut started, "racing tool").await;
        controller_clock.advance(20_000);
        cancel_observed
            .wait_for(|observed| *observed)
            .await
            .expect("cancellation observation channel stays open");
    });
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let audit = run(
        &model,
        tools,
        deadline_policy(20_000, None),
        clock,
        &cancellation,
    )
    .await;
    controller.await.expect("race controller");

    let messages = tool_messages(&audit);
    assert_eq!(messages.len(), 1);
    assert_eq!(
        messages[0].result,
        success_result("real completion"),
        "a deadline winner never overwrites an executor-proven completion"
    );
    assert_eq!(
        deadline_kinds(&audit, "call-race"),
        vec![ToolDeadlineKind::Hard]
    );
}

/// G (deadline side): the physical completion wins the arbitration first —
/// proven by parking after the physical branch won — so a hard deadline
/// that becomes eligible during the park never turns into intent: no
/// cancellation is requested, no intent fact is journaled, and the
/// completion result is untouched.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue204_physical_completion_winner_never_fires_a_deadline() {
    let scripted = call("call-fast", "tool-fast", "fast");
    let model = fake_model(tool_turn_then_stop(&[scripted]));
    let mut tools = ToolRegistry::new();
    let probe = register_probe(
        &mut tools,
        "fast",
        "tool-fast",
        ToolConcurrencyPolicy::Sequential,
        success_result("immediate"),
        &[],
        ProbeCancelSettlement::Cancelled,
        false,
    );
    let mut started = probe.started;
    let gates = probe.gates;
    let (pause, mut pause_reached, pause_release) =
        crate::agent::execution::test_sync::ToolPhysicalSettlementPause::install();

    let clock = Arc::new(ManualMonotonicClock::new());
    let controller_clock = clock.clone();
    let controller = tokio::spawn(async move {
        await_started(&mut started, "fast tool").await;
        gates[0].send_replace(true);
        pause_reached
            .wait_for(|reached| *reached)
            .await
            .expect("physical settlement pause channel stays open");
        // The hard deadline passes while the physical winner is parked: the
        // arbitration already linearized, so no intent may fire.
        controller_clock.advance(10_000);
        pause_release
            .send(())
            .expect("physical settlement pause remains installed");
    });

    let tool_runtime = common::tool_runtime(CONVERSATION);
    let store = tool_runtime.durable_store();
    let capability = common::capability_lease(tools, &tool_runtime).await;
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let mut execution = AgentExecution::new(
        request(&model),
        capability.into_lease(),
        &cancellation,
        execution_policy(deadline_policy(5_000, None), clock.clone()),
        context_runtime(&model, clock),
        &tool_runtime,
        rustx::agent::AttemptLifecycle::inert(),
    )
    .expect("conversation identity matches the tool runtime");
    execution.install_tool_physical_settlement_pause(pause);
    let result = tokio::time::timeout(GUARD, execution.run())
        .await
        .expect("Issue #204 execution must settle without wall-clock waiting");
    let audit = common::durable_agent_result(result, store.as_ref());
    controller.await.expect("physical winner controller");

    let messages = tool_messages(&audit);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].result, success_result("immediate"));
    assert!(
        deadline_kinds(&audit, "call-fast").is_empty(),
        "a completed execution never emits deadline intent"
    );
    assert_eq!(
        fact_sequence(&audit, "call-fast"),
        vec!["started", "completed"]
    );
}

/// K: repeated cancellation intent — the hard deadline first, then an
/// attempt-level cancellation while physical settlement is still in flight,
/// then further clock advances — settles the call exactly once. The
/// already-linearized deadline winner keeps the executor's honest
/// `OutcomeUnknown`; the attempt terminates cancelled without a second
/// turn.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue204_repeated_cancellation_intent_settles_exactly_once() {
    let scripted = call("call-repeat", "tool-repeat", "repeated");
    let model = fake_model(tool_turn_then_stop(&[scripted]));
    let mut tools = ToolRegistry::new();
    let probe = register_probe(
        &mut tools,
        "repeated",
        "tool-repeat",
        ToolConcurrencyPolicy::Sequential,
        success_result("not reached"),
        &[],
        ProbeCancelSettlement::OutcomeUnknown,
        true,
    );
    let mut started = probe.started;
    let mut cancel_observed = probe.cancel_observed;
    let settle_gate = probe.settle_gate.expect("settle gate installed");

    let clock = Arc::new(ManualMonotonicClock::new());
    let controller_clock = clock.clone();
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let controller_cancellation = cancellation.clone();
    let controller = tokio::spawn(async move {
        await_started(&mut started, "repeated tool").await;
        controller_clock.advance(20_000);
        cancel_observed
            .wait_for(|observed| *observed)
            .await
            .expect("cancellation observation channel stays open");
        // A second, attempt-level cancellation intent while the executor's
        // physical settlement is still in flight, plus further time.
        assert!(controller_cancellation.request_cancel(CancellationReason::UserRequested));
        controller_clock.advance(50_000);
        settle_gate.send_replace(true);
    });
    let audit = run(
        &model,
        tools,
        deadline_policy(20_000, None),
        clock,
        &cancellation,
    )
    .await;
    controller.await.expect("repeated intent controller");

    let messages = tool_messages(&audit);
    assert_eq!(messages.len(), 1, "exactly one canonical tool result");
    assert!(matches!(
        messages[0].result.status,
        ToolExecutionStatus::OutcomeUnknown { .. }
    ));
    assert_eq!(
        fact_sequence(&audit, "call-repeat"),
        vec!["started", "deadline:hard", "completed"],
        "one intent fact and one terminal fact, however many signals repeat"
    );
    assert_eq!(
        model.requests().len(),
        1,
        "the cancelled attempt has no next turn"
    );
    assert!(matches!(
        audit.result.outcome,
        AttemptOutcome::Cancelled {
            reason: CancellationReason::UserRequested
        }
    ));
}

/// J: one parallel batch where a sibling reaches its hard deadline while
/// the other completes normally. Every accepted call receives exactly one
/// canonical result, in model call order, and the deadline intent of one
/// call never touches its sibling.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue204_batch_siblings_settle_exactly_once_in_model_order() {
    let slow = call("call-slow", "tool-slow", "slow");
    let fast = call("call-fast-sibling", "tool-fast-sibling", "fast_sibling");
    let model = fake_model(tool_turn_then_stop(&[slow, fast]));
    let mut tools = ToolRegistry::new();
    let probe = register_probe(
        &mut tools,
        "slow",
        "tool-slow",
        ToolConcurrencyPolicy::Parallel,
        success_result("slow not reached"),
        &[],
        ProbeCancelSettlement::Cancelled,
        false,
    );
    let mut started = probe.started;
    FakeTool::new(
        common::tool_policies(
            "fast_sibling",
            "tool-fast-sibling",
            ToolExecutionPolicy::ForegroundOnly,
            ToolConcurrencyPolicy::Parallel,
        ),
        success_result("sibling done"),
    )
    .register(&mut tools);

    let clock = Arc::new(ManualMonotonicClock::new());
    let controller_clock = clock.clone();
    let controller = tokio::spawn(async move {
        await_started(&mut started, "slow sibling").await;
        controller_clock.advance(10_000);
    });
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let audit = run(
        &model,
        tools,
        deadline_policy(10_000, None),
        clock,
        &cancellation,
    )
    .await;
    controller.await.expect("batch controller");

    let messages = tool_messages(&audit);
    assert_eq!(
        messages.len(),
        2,
        "every accepted sibling settles exactly once"
    );
    assert_eq!(messages[0].tool_call_id, ToolCallId::new("call-slow"));
    assert_eq!(
        messages[1].tool_call_id,
        ToolCallId::new("call-fast-sibling"),
        "canonical result order remains the model call order"
    );
    assert!(matches!(
        messages[0].result.status,
        ToolExecutionStatus::TimedOut
    ));
    assert_eq!(messages[1].result, success_result("sibling done"));
    assert_eq!(
        fact_sequence(&audit, "call-slow"),
        vec!["started", "deadline:hard", "completed"]
    );
    assert_eq!(
        fact_sequence(&audit, "call-fast-sibling"),
        vec!["started", "completed"],
        "the sibling carries no deadline intent"
    );
    assert!(matches!(
        audit.result.outcome,
        AttemptOutcome::Completed {
            finish_reason: ModelFinishReason::Stop
        }
    ));
}

/// L (loop-level freeze): the admitted execution obeys the policy frozen in
/// its [`crate::agent::execution::AgentExecutionRuntimePolicy`]. Two
/// executions built from different policies are independent authorities: a
/// call admitted under a 10ms hard deadline settles `TimedOut` at t=10 even
/// though a second execution admits the same tool under a far larger one.
/// (The composition-level freeze — a reloaded resource generation never
/// reaching a running invocation — is covered by the configuration and
/// runtime-level regressions; `reload_resources` is attempt-exclusive, so a
/// mid-attempt reload is structurally refused.)
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue204_admitted_executions_obey_their_own_frozen_policy() {
    let scripted = call("call-frozen", "tool-frozen", "frozen");
    let model = fake_model(tool_turn_then_stop(&[scripted]));
    let (tool, release) = FakeTool::parking(
        common::tool("frozen", "tool-frozen"),
        success_result("released"),
    );
    let mut started = tool.started();
    let mut tools = ToolRegistry::new();
    tool.register(&mut tools);

    // Admission P1: hard deadline 10ms. The parked tool outlives it and
    // settles TimedOut.
    let clock_p1 = Arc::new(ManualMonotonicClock::new());
    let controller_clock = clock_p1.clone();
    let controller = tokio::spawn(async move {
        await_started(&mut started, "frozen tool").await;
        controller_clock.advance(10);
    });
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let audit_p1 = run(
        &model,
        tools,
        deadline_policy(10, None),
        clock_p1,
        &cancellation,
    )
    .await;
    controller.await.expect("P1 controller");
    let messages = tool_messages(&audit_p1);
    assert_eq!(messages.len(), 1);
    assert!(matches!(
        messages[0].result.status,
        ToolExecutionStatus::TimedOut
    ));

    // Admission P2 over the same tool: hard deadline one hour. Released
    // immediately, the call completes normally — P2 never rewrites P1's
    // already-committed result, and P1 never bounded P2.
    let scripted = call("call-frozen-2", "tool-frozen", "frozen");
    let model_p2 = fake_model(tool_turn_then_stop(&[scripted]));
    let (tool_p2, release_p2) = FakeTool::parking(
        common::tool("frozen", "tool-frozen"),
        success_result("released"),
    );
    let mut started_p2 = tool_p2.started();
    let mut registry_p2 = ToolRegistry::new();
    tool_p2.register(&mut registry_p2);
    drop(release);
    let clock_p2 = Arc::new(ManualMonotonicClock::new());
    let controller = tokio::spawn(async move {
        await_started(&mut started_p2, "frozen tool P2").await;
        release_p2.send_replace(true);
    });
    let cancellation_p2 = AgentCancellation::new(CancellationReason::UserRequested);
    let audit_p2 = run(
        &model_p2,
        registry_p2,
        deadline_policy(3_600_000, None),
        clock_p2,
        &cancellation_p2,
    )
    .await;
    controller.await.expect("P2 controller");
    let messages = tool_messages(&audit_p2);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].result, success_result("released"));
    assert!(deadline_kinds(&audit_p2, "call-frozen-2").is_empty());
}
