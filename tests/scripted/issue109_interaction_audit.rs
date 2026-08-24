//! Issue #109 (FND-04) — the durable interaction audit, proved through the
//! real Agent Loop and the real conversation runtime.
//!
//! The two planes this suite keeps apart are:
//!
//! ```text
//! pending waiter / prompt  = process-owned workflow state (never durable)
//! requested / settled fact = durable audit evidence (Event Journal)
//! ```
//!
//! Every ordering assertion reads the durable Event Journal by sequence, so
//! "before" means *committed at a lower durable sequence*, never "observed
//! first by a test thread". The required orders are
//!
//! ```text
//! InteractionRequested          <  the prompt reaching a client
//! InteractionSettled(Approved)  <  ToolExecutionStarted  <  external side effect
//! ```
//!
//! The store-facing half of the same contract — exactly-once settlement,
//! settled-without-requested, and the crash/restart behaviour of a historical
//! approval — lives in `tests/issue109_interaction_audit.rs`, which needs no
//! scripted model.
//!
//! There is no sleep and no timing assumption anywhere: the interaction
//! boundary is reached through the coordinator's own observation seam and
//! through watch channels.

use std::sync::{Arc, Mutex};

use futures_util::future::BoxFuture;
use rustx::agent::{
    AgentCancellation, AgentExecution, AgentExecutionRequest, AttemptLifecycle, LifecycleError,
    PreToolDecision, PreToolPolicy, PreToolView,
};
use rustx::context::{
    ContextAssembly, ContextRuntime, DefaultTokenEstimator, SessionContextPolicy,
};
use rustx::conversation::ConversationState;
use rustx::durable::{ConversationStore, SqliteConversationStore};
use rustx::events::types::{
    InteractionSettlement, InteractionSubject, RuntimeEvent, RuntimeEventEnvelope,
};
use rustx::message::content::TextBlock;
use rustx::message::types::{InboundKind, MessageBlock, UserContentBlock, UserMessageBlock};
use rustx::model::ModelFinishReason;
use rustx::runtime::identity::{
    AgentId, AttemptId, ConversationId, InteractionId, ToolCallId, ToolId,
};
use rustx::runtime::types::{CancellationReason, ConversationLifecycle};
use rustx::runtime::{
    ApprovalDecision, InteractionOutcome, InteractionRequest, InteractionResponse, QuestionAnswer,
};
use rustx::tools::executor::{ToolExecutionContext, ToolExecutor, ToolRegistry};
use rustx::tools::types::{
    ToolConcurrencyPolicy, ToolExecutionPolicy, ToolExecutionResult, ToolExecutionStatus,
    ToolInvocation,
};
use tokio::sync::watch;

use crate::runtime::interaction::{
    InteractionCoordinator, InteractionError, InteractionObserver, QuestionFacts,
};
use crate::scripted_suites::common;
use crate::scripted_suites::support;
use support::fake::{FakeModel, FakeStep, ScriptedCall, fake_model, tool_call_events};

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// The complete durable Event Journal of one conversation, in sequence order.
fn journal(store: &dyn ConversationStore) -> Vec<RuntimeEventEnvelope> {
    const PAGE: usize = 32;
    let mut cursor = None;
    let mut events = Vec::new();
    loop {
        let page = store.read_events(cursor, PAGE).expect("Event Journal page");
        if page.events.is_empty() {
            break;
        }
        events.extend(page.events);
        cursor = page.next_sequence;
    }
    events
}

/// The durable sequence of the first event matching `predicate`.
fn sequence_of(
    events: &[RuntimeEventEnvelope],
    predicate: impl Fn(&RuntimeEvent) -> bool,
) -> Option<u64> {
    events
        .iter()
        .find(|envelope| predicate(&envelope.event))
        .map(|envelope| envelope.sequence)
}

fn interaction_facts(events: &[RuntimeEventEnvelope]) -> Vec<RuntimeEvent> {
    events
        .iter()
        .filter(|envelope| {
            matches!(
                envelope.event,
                RuntimeEvent::InteractionRequested { .. } | RuntimeEvent::InteractionSettled { .. }
            )
        })
        .map(|envelope| envelope.event.clone())
        .collect()
}

/// A pre-tool policy that returns one scripted decision per evaluated call.
struct ScriptedAskPolicy {
    decisions: Mutex<std::collections::VecDeque<PreToolDecision>>,
}

impl ScriptedAskPolicy {
    fn ask(reason: &str) -> Arc<Self> {
        Arc::new(Self {
            decisions: Mutex::new(
                vec![PreToolDecision::Ask {
                    reason: reason.to_owned(),
                }]
                .into(),
            ),
        })
    }
}

impl PreToolPolicy for ScriptedAskPolicy {
    fn evaluate<'a>(
        &'a self,
        _view: &'a PreToolView<'a>,
    ) -> BoxFuture<'a, Result<PreToolDecision, LifecycleError>> {
        let decision = self
            .decisions
            .lock()
            .expect("scripted decision lock")
            .pop_front()
            .unwrap_or(PreToolDecision::Allow);
        Box::pin(async move { Ok(decision) })
    }
}

/// A tool that records every invocation it actually received.
struct SpyTool {
    invocations: Arc<Mutex<Vec<ToolInvocation>>>,
}

impl SpyTool {
    fn register(
        registry: &mut ToolRegistry,
        name: &str,
        id: &str,
    ) -> Arc<Mutex<Vec<ToolInvocation>>> {
        let invocations = Arc::new(Mutex::new(Vec::new()));
        let definition = common::tool_policies(
            name,
            id,
            ToolExecutionPolicy::ForegroundOnly,
            ToolConcurrencyPolicy::Sequential,
        );
        registry
            .register(
                definition,
                Arc::new(Self {
                    invocations: Arc::clone(&invocations),
                }),
            )
            .expect("spy tool registration");
        invocations
    }
}

impl ToolExecutor for SpyTool {
    fn execute<'a>(
        &'a self,
        invocation: ToolInvocation,
        _context: ToolExecutionContext<'a>,
    ) -> BoxFuture<'a, ToolExecutionResult> {
        self.invocations
            .lock()
            .expect("spy invocations lock")
            .push(invocation);
        Box::pin(async move {
            ToolExecutionResult {
                status: ToolExecutionStatus::Success,
                content: Vec::new(),
                duration_ms: 0,
                exit_code: Some(0),
                artifacts: Vec::new(),
                truncation: None,
                managed_output: None,
            }
        })
    }
}

/// A coordinator observer that answers the first interaction it is shown, and
/// records what the durable Journal already contained at the exact instant the
/// prompt was released.
struct RespondingObserver {
    coordinator: Mutex<Option<Arc<InteractionCoordinator>>>,
    store: Arc<SqliteConversationStore>,
    response: Mutex<Option<InteractionResponse>>,
    /// The interaction facts durable at the moment `on_pending` ran.
    durable_at_prompt: Mutex<Vec<RuntimeEvent>>,
    prompts: Mutex<Vec<InteractionRequest>>,
    settled: Mutex<Vec<(InteractionId, InteractionOutcome)>>,
    published: watch::Sender<usize>,
}

impl RespondingObserver {
    fn new(
        store: Arc<SqliteConversationStore>,
        response: Option<InteractionResponse>,
    ) -> Arc<Self> {
        Arc::new(Self {
            coordinator: Mutex::new(None),
            store,
            response: Mutex::new(response),
            durable_at_prompt: Mutex::new(Vec::new()),
            prompts: Mutex::new(Vec::new()),
            settled: Mutex::new(Vec::new()),
            published: watch::channel(0).0,
        })
    }

    fn bind(&self, coordinator: Arc<InteractionCoordinator>) {
        *self.coordinator.lock().expect("bind lock") = Some(coordinator);
    }

    fn prompts(&self) -> Vec<InteractionRequest> {
        self.prompts.lock().expect("prompt lock").clone()
    }

    fn durable_at_prompt(&self) -> Vec<RuntimeEvent> {
        self.durable_at_prompt
            .lock()
            .expect("durable-at-prompt lock")
            .clone()
    }

    fn settled(&self) -> Vec<(InteractionId, InteractionOutcome)> {
        self.settled.lock().expect("settled lock").clone()
    }
}

impl InteractionObserver for RespondingObserver {
    fn on_pending(&self, request: &InteractionRequest) {
        // The durable read happens *inside* the prompt-release callback, so
        // what it sees is exactly what was committed before any client could
        // learn the prompt exists.
        *self
            .durable_at_prompt
            .lock()
            .expect("durable-at-prompt lock") = interaction_facts(&journal(self.store.as_ref()));
        let count = {
            let mut prompts = self.prompts.lock().expect("prompt lock");
            prompts.push(request.clone());
            prompts.len()
        };
        self.published.send_replace(count);
        // Answering from the release callback would re-enter the coordinator
        // under its own lock; the answer is delivered by the driving task
        // instead. This callback stays a leaf publication.
    }

    fn on_settled(&self, interaction_id: &InteractionId, outcome: &InteractionOutcome) {
        self.settled
            .lock()
            .expect("settled lock")
            .push((interaction_id.clone(), outcome.clone()));
    }
}

fn scripted_call(id: &'static str, tool_id: &'static str, name: &'static str) -> ScriptedCall {
    ScriptedCall {
        id,
        tool_id,
        name,
        arguments: serde_json::json!({}),
    }
}

/// A model script: one tool-call turn, then one plain stop turn.
fn tool_turn_then_stop(calls: &[ScriptedCall]) -> Vec<Vec<FakeStep>> {
    let mut first = vec![FakeStep::Emit(rustx::model::event::ModelEvent::Started)];
    for (index, call) in calls.iter().enumerate() {
        for event in tool_call_events(u32::try_from(index).expect("small batch"), call) {
            first.push(FakeStep::Emit(event));
        }
    }
    first.push(FakeStep::Emit(rustx::model::event::ModelEvent::Completed {
        finish_reason: ModelFinishReason::ToolCalls,
        usage: None,
    }));
    vec![
        first,
        vec![
            FakeStep::Emit(rustx::model::event::ModelEvent::Started),
            FakeStep::Emit(rustx::model::event::ModelEvent::TextDelta {
                block_index: rustx::message::types::ContentBlockIndex::new(0),
                text: "done".to_owned(),
            }),
            FakeStep::Emit(rustx::model::event::ModelEvent::Completed {
                finish_reason: ModelFinishReason::Stop,
                usage: None,
            }),
        ],
    ]
}

fn request(conversation_id: ConversationId, model: &Arc<FakeModel>) -> AgentExecutionRequest {
    AgentExecutionRequest {
        agent_id: AgentId::new("agent-a"),
        conversation_id,
        attempt_id: AttemptId::new("attempt-1"),
        conversation: ConversationState::from_messages(vec![MessageBlock::User(
            UserMessageBlock {
                id: rustx::runtime::identity::MessageId::new("msg-user-1"),
                content: vec![UserContentBlock::Text(TextBlock {
                    text: "go".to_owned(),
                })],
                source: rustx::message::types::UserSource::Human,
                kind: InboundKind::Message,
                timestamp: None,
            },
        )])
        .expect("bootstrap conversation"),
        initial_turn_trigger: rustx::agent::InitialTurnTrigger::Continuation,
        timezone: None,
        model: support::attempt_model(model.clone(), "fake-model"),
    }
}

fn context_runtime(model: &Arc<FakeModel>) -> ContextRuntime {
    ContextRuntime::for_attempt_with_assembly(
        SessionContextPolicy {
            reserve_tokens: 0,
            keep_recent_tokens: 0,
            summary_output_cap: None,
        },
        Arc::new(DefaultTokenEstimator),
        rustx::context::AgentStatusComposer::default(),
        ContextAssembly::new(),
        &support::attempt_model(model.clone(), "fake-model"),
    )
    .expect("valid context runtime")
}

/// One attempt driven through a real coordinator over a real durable store.
struct Run {
    store: Arc<SqliteConversationStore>,
    events: Vec<RuntimeEventEnvelope>,
    observer: Arc<RespondingObserver>,
    result: rustx::agent::AgentExecutionResult,
}

/// Runs one approval attempt: the scripted policy asks, the observer records
/// the prompt, and the driving task delivers `response` once the coordinator
/// has published exactly one pending interaction.
async fn run_approval(
    conversation: &str,
    response: Option<InteractionResponse>,
) -> (Run, Arc<Mutex<Vec<ToolInvocation>>>) {
    let store = Arc::new(
        SqliteConversationStore::in_memory(ConversationId::new(conversation)).expect("store"),
    );
    let erased: Arc<dyn ConversationStore> = store.clone();
    let fixture = common::tool_runtime_with_store(conversation, Some(erased.clone()));
    let tool_runtime: rustx::tools::runtime::ConversationToolRuntime = (*fixture).clone();

    let mut tools = ToolRegistry::new();
    let invocations = SpyTool::register(&mut tools, "alpha", "tool-alpha");
    let capability = common::capability_lease(tools, &tool_runtime).await;

    let lifecycle = ConversationLifecycle::new();
    assert!(lifecycle.activate());
    let coordinator = Arc::new(InteractionCoordinator::new(
        ConversationId::new(conversation),
        lifecycle,
        rustx::durable::interaction_audit_capability(erased),
    ));
    coordinator.set_provider_available(true);
    let observer = RespondingObserver::new(store.clone(), response.clone());
    observer.bind(coordinator.clone());
    coordinator.install_observer(observer.clone());
    let mut published = observer.published.subscribe();

    let model = fake_model(tool_turn_then_stop(&[scripted_call(
        "call-approval",
        "tool-alpha",
        "alpha",
    )]));
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);

    // The responder waits on the coordinator's own publication signal, so the
    // answer is delivered strictly after the prompt was released.
    let responder = {
        let coordinator = coordinator.clone();
        let observer = observer.clone();
        tokio::spawn(async move {
            published
                .wait_for(|count| *count == 1)
                .await
                .expect("the coordinator published one prompt");
            let Some(response) = observer.response.lock().expect("response lock").take() else {
                return;
            };
            let id = observer.prompts()[0].id.clone();
            coordinator
                .respond(&id, response)
                .expect("the coordinator accepts the scripted response");
        })
    };

    let result = AgentExecution::new(
        request(ConversationId::new(conversation), &model),
        capability.into_lease(),
        &cancellation,
        context_runtime(&model),
        &tool_runtime,
        AttemptLifecycle::inert()
            .with_pre_tool_policy(ScriptedAskPolicy::ask("issue109 approval"))
            .with_native_interaction(coordinator.clone()),
    )
    .expect("conversation identity matches the tool runtime")
    .run()
    .await;
    responder.await.expect("responder task");

    let events = journal(store.as_ref());
    (
        Run {
            store,
            events,
            observer,
            result,
        },
        invocations,
    )
}

// ---------------------------------------------------------------------------
// Regression 1 — durable before the prompt
// ---------------------------------------------------------------------------

/// **Regression 1.** `InteractionRequested` commits before the user-facing
/// prompt is published.
///
/// The proof is taken inside the prompt-release callback itself: the durable
/// Journal already contains the requested fact for this exact interaction at
/// the instant a client could first learn the prompt exists.
#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn requested_fact_commits_before_the_prompt_reaches_a_client() {
    let (run, _) = run_approval(
        "conv-109-before-prompt",
        Some(InteractionResponse::Approval {
            decision: ApprovalDecision::Allow,
        }),
    )
    .await;

    let prompts = run.observer.prompts();
    assert_eq!(prompts.len(), 1, "exactly one prompt was released");
    let interaction_id = prompts[0].id.clone();

    let at_prompt = run.observer.durable_at_prompt();
    assert!(
        at_prompt.iter().any(|event| matches!(
            event,
            RuntimeEvent::InteractionRequested { interaction_id: id, .. } if *id == interaction_id
        )),
        "the requested fact must already be durable when the prompt is released, saw {at_prompt:?}"
    );
    assert!(
        at_prompt
            .iter()
            .all(|event| !matches!(event, RuntimeEvent::InteractionSettled { .. })),
        "no settlement exists yet at prompt release"
    );

    // The audit subject is by-value and independent of the tool registry.
    let subject = run
        .events
        .iter()
        .find_map(|envelope| match &envelope.event {
            RuntimeEvent::InteractionRequested { subject, .. } => Some(subject.clone()),
            _ => None,
        })
        .expect("one requested fact");
    assert!(matches!(
        subject,
        InteractionSubject::Approval {
            ref call_id,
            ref tool_id,
            ref tool_name,
            ref reason,
            ..
        } if call_id == &ToolCallId::new("call-approval")
            && tool_id == &ToolId::new("tool-alpha")
            && tool_name == "alpha"
            && reason == "issue109 approval"
    ));
}

// ---------------------------------------------------------------------------
// Regression 2 — approval ordering
// ---------------------------------------------------------------------------

/// **Regression 2.** `InteractionSettled(Approved)` commits before
/// `ToolExecutionStarted`, which itself commits before the executor runs.
///
/// The comparison is on durable sequences, so the user-facing approval
/// response provably cannot race ahead of the evidence that the approval
/// existed.
#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn approved_settlement_commits_before_tool_execution_started() {
    let (run, invocations) = run_approval(
        "conv-109-approval-order",
        Some(InteractionResponse::Approval {
            decision: ApprovalDecision::Allow,
        }),
    )
    .await;

    let requested = sequence_of(&run.events, |event| {
        matches!(event, RuntimeEvent::InteractionRequested { .. })
    })
    .expect("a requested fact is durable");
    let settled = sequence_of(&run.events, |event| {
        matches!(
            event,
            RuntimeEvent::InteractionSettled {
                settlement: InteractionSettlement::Approved,
                ..
            }
        )
    })
    .expect("an approved settlement is durable");
    let started = sequence_of(&run.events, |event| {
        matches!(event, RuntimeEvent::ToolExecutionStarted { .. })
    })
    .expect("the approved tool started");

    assert!(
        requested < settled && settled < started,
        "required order is requested < settled(approved) < tool start, got {requested} {settled} {started}"
    );
    assert_eq!(
        invocations.lock().expect("spy lock").len(),
        1,
        "the executor ran exactly once, after its durable start fact"
    );
    assert!(matches!(
        run.observer.settled().as_slice(),
        [(_, InteractionOutcome::Answered { .. })]
    ));
}

// ---------------------------------------------------------------------------
// Regression 5 — denial
// ---------------------------------------------------------------------------

/// **Regression 5.** A denial remains the canonical denied `ToolResult` and
/// carries a matching interaction audit; the executor never runs and no
/// `ToolExecutionStarted` fact exists.
#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn denial_is_a_denied_tool_result_with_matching_interaction_audit() {
    let (run, invocations) = run_approval(
        "conv-109-denial",
        Some(InteractionResponse::Approval {
            decision: ApprovalDecision::Deny {
                reason: "denied by the operator".to_owned(),
            },
        }),
    )
    .await;

    assert!(
        invocations.lock().expect("spy lock").is_empty(),
        "a denied call never reaches its executor"
    );
    assert!(
        sequence_of(&run.events, |event| matches!(
            event,
            RuntimeEvent::ToolExecutionStarted { .. }
        ))
        .is_none(),
        "a denied call has no durable start fact"
    );

    let facts = interaction_facts(&run.events);
    assert!(matches!(
        facts.as_slice(),
        [
            RuntimeEvent::InteractionRequested { .. },
            RuntimeEvent::InteractionSettled {
                settlement: InteractionSettlement::Denied { reason },
                ..
            }
        ] if reason == "denied by the operator"
    ));

    let denied = run
        .result
        .messages()
        .iter()
        .filter_map(|message| match message {
            MessageBlock::Tool(tool) => Some(tool.result.status.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(
        matches!(denied.as_slice(), [ToolExecutionStatus::Denied { .. }]),
        "the canonical result slot stays a denied ToolResult, got {denied:?}"
    );
}

// ---------------------------------------------------------------------------
// Regression 8 — exactly-once settlement
// ---------------------------------------------------------------------------

/// **Regression 8.** One interaction identity settles exactly once. The
/// duplicate response is refused by the live coordinator, and the durable
/// authority independently refuses a second settled fact for that identity.
#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn interaction_settlement_is_exactly_once_for_one_identity() {
    let (run, _) = run_approval(
        "conv-109-exactly-once",
        Some(InteractionResponse::Approval {
            decision: ApprovalDecision::Allow,
        }),
    )
    .await;
    let interaction_id = run.observer.prompts()[0].id.clone();

    assert_eq!(
        interaction_facts(&run.events).len(),
        2,
        "one requested fact and one settled fact"
    );

    // The durable authority is the second, independent guard: replaying the
    // exact settled envelope is a typed terminal violation, not a duplicate.
    let settled = run
        .events
        .iter()
        .find(|envelope| matches!(envelope.event, RuntimeEvent::InteractionSettled { .. }))
        .cloned()
        .expect("one settled envelope");
    let replay = RuntimeEventEnvelope {
        sequence: 0,
        ..settled
    };
    assert!(
        matches!(
            rustx::durable::ConversationStore::append_event(run.store.as_ref(), replay),
            Err(rustx::durable::ConversationStoreError::TerminalViolation(_))
        ),
        "the durable authority refuses a second settlement of {interaction_id}"
    );
}

// ---------------------------------------------------------------------------
// Regression 9 — headless execution and client presence
// ---------------------------------------------------------------------------

/// **Regression 9 (headless half).** With no interaction-capable client the
/// approval fails closed as a denial *and* no interaction audit is written at
/// all: rustX never records that a user was asked something no user saw.
#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn a_headless_attempt_records_no_interaction_audit_and_fails_closed() {
    let conversation = "conv-109-headless";
    let store = Arc::new(
        SqliteConversationStore::in_memory(ConversationId::new(conversation)).expect("store"),
    );
    let erased: Arc<dyn ConversationStore> = store.clone();
    let fixture = common::tool_runtime_with_store(conversation, Some(erased.clone()));
    let tool_runtime: rustx::tools::runtime::ConversationToolRuntime = (*fixture).clone();
    let mut tools = ToolRegistry::new();
    let invocations = SpyTool::register(&mut tools, "alpha", "tool-alpha");
    let capability = common::capability_lease(tools, &tool_runtime).await;

    let lifecycle = ConversationLifecycle::new();
    assert!(lifecycle.activate());
    let coordinator = Arc::new(InteractionCoordinator::new(
        ConversationId::new(conversation),
        lifecycle,
        rustx::durable::interaction_audit_capability(erased),
    ));
    // No capable client is attached.
    coordinator.set_provider_available(false);

    let model = fake_model(tool_turn_then_stop(&[scripted_call(
        "call-headless",
        "tool-alpha",
        "alpha",
    )]));
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let result = AgentExecution::new(
        request(ConversationId::new(conversation), &model),
        capability.into_lease(),
        &cancellation,
        context_runtime(&model),
        &tool_runtime,
        AttemptLifecycle::inert()
            .with_pre_tool_policy(ScriptedAskPolicy::ask("issue109 headless"))
            .with_native_interaction(coordinator.clone()),
    )
    .expect("conversation identity matches the tool runtime")
    .run()
    .await;

    assert!(
        invocations.lock().expect("spy lock").is_empty(),
        "an unanswerable approval fails closed"
    );
    assert!(
        interaction_facts(&journal(store.as_ref())).is_empty(),
        "a prompt no client could ever see leaves no audit record"
    );
    assert!(
        result.messages().iter().any(|message| matches!(
            message,
            MessageBlock::Tool(tool)
                if matches!(tool.result.status, ToolExecutionStatus::Denied { .. })
        )),
        "the canonical slot is the existing fail-closed denial"
    );
}

/// **Regression 9 (detach/reattach half).** A client that detaches after the
/// prompt was released does not settle the live interaction, the durable audit
/// is untouched by the detach, and a reattached client can still answer the
/// same interaction identity exactly once.
#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn interaction_audit_survives_client_detach_and_reattach() {
    let conversation = "conv-109-detach";
    let store = Arc::new(
        SqliteConversationStore::in_memory(ConversationId::new(conversation)).expect("store"),
    );
    let erased: Arc<dyn ConversationStore> = store.clone();
    let lifecycle = ConversationLifecycle::new();
    assert!(lifecycle.activate());
    let coordinator = Arc::new(InteractionCoordinator::new(
        ConversationId::new(conversation),
        lifecycle,
        rustx::durable::interaction_audit_capability(erased),
    ));
    coordinator.set_provider_available(true);
    let observer = RespondingObserver::new(store.clone(), None);
    coordinator.install_observer(observer.clone());

    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let waiter = {
        let coordinator = coordinator.clone();
        let cancellation = cancellation.execution_cancellation();
        tokio::spawn(async move {
            coordinator
                .request_question(
                    AttemptId::new("attempt-detach"),
                    QuestionFacts {
                        turn: 1,
                        prompt: "Which target?".to_owned(),
                        choices: Some(vec!["staging".to_owned(), "production".to_owned()]),
                        allow_free_text: false,
                    },
                    cancellation,
                )
                .await
        })
    };
    let mut published = observer.published.subscribe();
    published
        .wait_for(|count| *count == 1)
        .await
        .expect("one prompt published");
    let interaction_id = observer.prompts()[0].id.clone();

    let after_request = interaction_facts(&journal(store.as_ref()));
    assert_eq!(after_request.len(), 1, "only the requested fact so far");

    // The client detaches. The live interaction is untouched, and so is the
    // durable audit: detach is a client fact, not a settlement.
    coordinator.set_provider_available(false);
    assert_eq!(coordinator.pending_count(), 1);
    assert_eq!(interaction_facts(&journal(store.as_ref())), after_request);

    // A client reattaches and answers the very same identity.
    coordinator.set_provider_available(true);
    coordinator
        .respond(
            &interaction_id,
            InteractionResponse::Question {
                answer: QuestionAnswer::Choice {
                    value: "staging".to_owned(),
                },
            },
        )
        .expect("the reattached client answers the live interaction");
    assert_eq!(
        waiter.await.expect("question waiter"),
        InteractionOutcome::Answered {
            response: InteractionResponse::Question {
                answer: QuestionAnswer::Choice {
                    value: "staging".to_owned(),
                },
            },
        }
    );

    let facts = interaction_facts(&journal(store.as_ref()));
    assert!(matches!(
        facts.as_slice(),
        [
            RuntimeEvent::InteractionRequested {
                subject: InteractionSubject::Question { prompt, .. },
                ..
            },
            RuntimeEvent::InteractionSettled {
                settlement: InteractionSettlement::Answered {
                    answer: QuestionAnswer::Choice { value }
                },
                ..
            }
        ] if prompt == "Which target?" && value == "staging"
    ));
    assert_eq!(
        coordinator.respond(
            &interaction_id,
            InteractionResponse::Question {
                answer: QuestionAnswer::FreeText {
                    value: "late".to_owned(),
                },
            },
        ),
        Err(InteractionError::NotPending {
            interaction_id: interaction_id.clone()
        }),
        "the live operation receives exactly one answer"
    );
}
