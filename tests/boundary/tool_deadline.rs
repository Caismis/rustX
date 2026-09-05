//! Issue #204, case E: a real foreground Bash execution bounded by the
//! generic hard deadline settles as the *proven* timeout.
//!
//! The Agent Loop owns the deadline; the Bash supervisor owns physical
//! settlement. When the manual clock crosses the hard deadline, the loop
//! requests physical cancellation of exactly this execution and awaits the
//! supervisor's settlement. The supervisor kills the owned process group
//! and reaps it (the real process boundary this suite exists to exercise),
//! so the executor-proven `Cancelled` is normalized to `TimedOut` — never
//! a dropped future pretending to be settlement. Wall-clock time appears
//! only as the outer anti-hang guard; the deadline itself is driven by the
//! manual monotonic clock.

#![cfg(unix)]

use super::{common, support};

use std::sync::Arc;
use std::time::Duration;

use rustx::agent::{
    AgentCancellation, AgentExecution, AgentExecutionObserver, AgentExecutionRequest,
    AgentStatusObservation,
};
use rustx::durable::TranscriptCursor;
use rustx::events::types::RuntimeEvent;
use rustx::message::content::TextBlock;
use rustx::message::types::{MessageBlock, UserContentBlock, UserMessageBlock, UserSource};
use rustx::model::event::ModelEvent;
use rustx::model::finish::ModelFinishReason;
use rustx::publication::{PublicationAudit, PublicationFrame, PublicationStreamStart};
use rustx::runtime::identity::{AgentId, AttemptId, MessageId};
use rustx::runtime::types::CancellationReason;
use rustx::runtime::{ManualMonotonicClock, MonotonicClock};
use rustx::tools::deadline::{ToolDeadlineKind, ToolExecutionDeadlinePolicy};
use rustx::tools::types::ToolExecutionStatus;
use support::fake::{FakeModel, FakeStep, ScriptedCall, fake_model, tool_call_events};
use tokio::sync::watch;

/// Signals when the durable `ToolExecutionStarted` fact of the Bash call is
/// observed, so the test controller advances the clock only after the
/// execution provably started.
struct StartedSignal {
    started: watch::Sender<bool>,
}

impl AgentExecutionObserver for StartedSignal {
    fn observe_event(&self, _attempt_id: &AttemptId, event: &RuntimeEvent) {
        if matches!(event, RuntimeEvent::ToolExecutionStarted { .. }) {
            self.started.send_replace(true);
        }
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

/// A foreground Bash `sleep 30` admitted under a 5-second hard deadline is
/// killed and reaped by the supervisor after the deadline's cancellation
/// intent, and the one canonical result is `TimedOut` — proven physical
/// settlement, not `OutcomeUnknown`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue204_bash_hard_deadline_settles_proven_timed_out() {
    // The fixture stays whole: its private TempDir field owns the workspace
    // the supervisor spawns into.
    let fixture = common::native_fixture();
    let registry = fixture.registry;
    let bash_id = registry
        .definitions()
        .into_iter()
        .find(|definition| definition.name == "bash")
        .expect("native bash tool registered")
        .id
        .as_str()
        .to_owned();
    let scripted = ScriptedCall {
        id: "call-bash-deadline",
        tool_id: Box::leak(bash_id.into_boxed_str()),
        name: "bash",
        arguments: serde_json::json!({"command": "sleep 30"}),
    };
    let mut first = vec![FakeStep::Emit(ModelEvent::Started)];
    for event in tool_call_events(0, &scripted) {
        first.push(FakeStep::Emit(event));
    }
    first.push(FakeStep::Emit(ModelEvent::Completed {
        finish_reason: ModelFinishReason::ToolCalls,
        usage: None,
    }));
    let model: Arc<FakeModel> = fake_model(vec![
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
    ]);

    let capability = common::capability_lease(registry, &fixture.runtime).await;
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
            agent_id: AgentId::new("agent-204-boundary"),
            conversation_id: fixture.runtime.conversation_id().clone(),
            attempt_id: AttemptId::new("attempt-204-boundary"),
            conversation: rustx::conversation::ConversationState::from_messages(vec![
                MessageBlock::User(UserMessageBlock {
                    id: MessageId::new("msg-user-204-boundary"),
                    content: vec![UserContentBlock::Text(TextBlock {
                        text: "sleep".to_owned(),
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
        capability.into_lease(),
        &cancellation,
        crate::agent::execution::AgentExecutionRuntimePolicy {
            model_timeout_policy: rustx::model::ModelTimeoutPolicy::default(),
            tool_deadline_policy: ToolExecutionDeadlinePolicy {
                hard_deadline: Duration::from_secs(5),
                idle_liveness: None,
            },
            monotonic_clock: clock.clone() as Arc<dyn MonotonicClock>,
            subagent_context: None,
            workflow_output: None,
        },
        context_runtime,
        &fixture.runtime,
        rustx::agent::AttemptLifecycle::inert(),
    )
    .expect("conversation identity matches the tool runtime");
    let (started_sender, mut started) = watch::channel(false);
    let observer = StartedSignal {
        started: started_sender,
    };
    execution.observe(&observer);

    let controller_clock = clock.clone();
    let controller = tokio::spawn(async move {
        started
            .wait_for(|is_started| *is_started)
            .await
            .expect("tool start observation channel stays open");
        controller_clock.advance(5_000);
    });
    let result = tokio::time::timeout(Duration::from_secs(30), execution.run())
        .await
        .expect("Bash deadline settlement must not wait out the real sleep");
    controller.await.expect("boundary deadline controller");
    let audit = common::durable_agent_result(result, fixture.store.as_ref());

    let messages: Vec<_> = audit
        .result
        .messages()
        .iter()
        .filter_map(|message| match message {
            MessageBlock::Tool(tool) => Some(tool),
            _ => None,
        })
        .collect();
    assert_eq!(messages.len(), 1, "exactly one canonical tool result");
    assert!(
        matches!(messages[0].result.status, ToolExecutionStatus::TimedOut),
        "the supervisor killed and reaped the process group after the hard \
         deadline, so the proven timeout is TimedOut: {:?}",
        messages[0].result.status
    );
    let facts: Vec<&RuntimeEvent> = audit
        .event_history
        .iter()
        .filter(|event| {
            matches!(
                event,
                RuntimeEvent::ToolExecutionStarted { .. }
                    | RuntimeEvent::ToolExecutionDeadlineFired { .. }
                    | RuntimeEvent::ToolExecutionCompleted { .. }
            )
        })
        .collect();
    assert_eq!(facts.len(), 3, "start, intent, terminal — exactly once");
    assert!(matches!(
        facts[0],
        RuntimeEvent::ToolExecutionStarted { .. }
    ));
    assert!(matches!(
        facts[1],
        RuntimeEvent::ToolExecutionDeadlineFired {
            kind: ToolDeadlineKind::Hard,
            ..
        }
    ));
    assert!(matches!(
        facts[2],
        RuntimeEvent::ToolExecutionCompleted { .. }
    ));
}
