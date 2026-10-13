//! Native questionnaire argument integrity and rejection feedback through the
//! ordinary Agent Loop. All model turns and human answers are scripted; the
//! real registry, durable history, and interaction coordinator own the path.

use std::sync::{Arc, Mutex};

use rustx::agent::{AgentCancellation, AgentExecution, AgentExecutionRequest, AttemptLifecycle};
use rustx::durable::ConversationStore;
use rustx::events::types::{AttemptOutcome, RuntimeEvent, RuntimeEventEnvelope};
use rustx::events::{
    AnswerSpecification, InteractionSettlement, InteractionSubject, OptionAnswer,
    OptionSpecification, QuestionSpecification, QuestionnaireAnswer, QuestionnaireAnswerEntry,
    QuestionnaireResponse, QuestionnaireSpecification, QuestionnaireSubmission,
    SingleChoiceSpecification,
};
use rustx::message::types::{
    AssistantContentBlock, ContentBlockIndex, MessageBlock, ToolMessageBlock, UserSource,
};
use rustx::model::{ModelEvent, ModelFinishReason, ModelRequest};
use rustx::runtime::identity::{AgentId, AttemptId, InteractionId, ToolCallId, ToolId};
use rustx::runtime::types::{CancellationReason, ConversationLifecycle};
use rustx::runtime::{
    InteractionKind, InteractionOutcome, InteractionRequest, InteractionResponse,
};
use rustx::tools::executor::PreflightOutcome;
use rustx::tools::types::{ToolCall, ToolCallStart, ToolExecutionStatus, ToolResultContent};
use serde_json::{Value, json};
use tokio::sync::watch;

use crate::runtime::interaction::{InteractionCoordinator, InteractionObserver};
use crate::scripted_suites::{common, support};
use support::fake::{FakeModel, FakeStep, fake_model, model_release};

fn questionnaire_arguments() -> Value {
    json!({
        "questions": [{
            "question": "您希望首页采用哪一种视觉风格？",
            "header": "首页风格",
            "options": [
                {
                    "label": "清晰简洁",
                    "description": "突出核心信息，减少视觉干扰。",
                    "preview": "## 首页预览\n欢迎探索我们的产品。"
                },
                {
                    "label": "生动叙事",
                    "description": "以丰富的图像和故事展示产品特色。"
                },
                {
                    "label": "专业可信",
                    "description": "通过案例与数据建立专业形象。"
                }
            ]
        }]
    })
}

fn expected_questionnaire() -> QuestionnaireSpecification {
    QuestionnaireSpecification {
        questions: vec![QuestionSpecification {
            question: "您希望首页采用哪一种视觉风格？".to_owned(),
            header: "首页风格".to_owned(),
            answer: AnswerSpecification::SingleChoice(SingleChoiceSpecification {
                options: vec![
                    OptionSpecification {
                        label: "清晰简洁".to_owned(),
                        description: "突出核心信息，减少视觉干扰。".to_owned(),
                        preview: Some("## 首页预览\n欢迎探索我们的产品。".to_owned()),
                    },
                    OptionSpecification {
                        label: "生动叙事".to_owned(),
                        description: "以丰富的图像和故事展示产品特色。".to_owned(),
                        preview: None,
                    },
                    OptionSpecification {
                        label: "专业可信".to_owned(),
                        description: "通过案例与数据建立专业形象。".to_owned(),
                        preview: None,
                    },
                ],
                allow_custom: true,
            }),
        }],
    }
}

fn call(id: &str, arguments: Value) -> ToolCall {
    ToolCall {
        id: ToolCallId::new(id),
        tool_id: ToolId::new("tool-ask-user"),
        name: "ask_user".to_owned(),
        arguments,
    }
}

/// Simulates fragmented argument streaming with one Unicode scalar per delta,
/// including splits inside every authored string. The completion is parsed
/// from those exact fragments by the production proposal acceptance boundary,
/// rather than supplying an unrelated preconstructed completed JSON value.
fn streamed_turn(call: &ToolCall) -> Vec<FakeStep> {
    let start = ToolCallStart {
        id: call.id.clone(),
        tool_id: call.tool_id.clone(),
        name: call.name.clone(),
    };
    let mut steps = vec![
        FakeStep::Emit(ModelEvent::Started),
        FakeStep::Emit(ModelEvent::ToolCallStarted {
            block_index: ContentBlockIndex::new(0),
            call: start.clone(),
        }),
    ];
    let encoded = serde_json::to_string(&call.arguments).expect("fixture arguments serialize");
    let mut assembled = String::new();
    for character in encoded.chars() {
        let fragment = character.to_string();
        assembled.push_str(&fragment);
        steps.push(FakeStep::Emit(ModelEvent::ToolCallArgumentsDelta {
            block_index: ContentBlockIndex::new(0),
            call_id: call.id.clone(),
            arguments_delta: fragment,
        }));
    }
    let completed = crate::model::adapter::proposal::accept_tool_call_arguments(&start, &assembled)
        .expect("the fragmented Unicode arguments deserialize");
    assert_eq!(
        &completed, call,
        "stream assembly preserves the exact fixture"
    );
    steps.extend([
        FakeStep::Emit(ModelEvent::ToolCallCompleted {
            block_index: ContentBlockIndex::new(0),
            call: completed,
        }),
        FakeStep::Emit(ModelEvent::Completed {
            finish_reason: ModelFinishReason::ToolCalls,
            usage: None,
        }),
    ]);
    steps
}

fn stop_turn() -> Vec<FakeStep> {
    vec![
        FakeStep::Emit(ModelEvent::Started),
        FakeStep::Emit(ModelEvent::TextDelta {
            block_index: ContentBlockIndex::new(0),
            text: "The questionnaire answer was received.".to_owned(),
        }),
        FakeStep::Emit(ModelEvent::Completed {
            finish_reason: ModelFinishReason::Stop,
            usage: None,
        }),
    ]
}

struct QuestionnaireObserver {
    published: watch::Sender<Vec<InteractionRequest>>,
    settled: Mutex<Vec<(InteractionId, InteractionOutcome)>>,
}

impl InteractionObserver for QuestionnaireObserver {
    fn on_pending(
        &self,
        request: &InteractionRequest,
        _audit: &RuntimeEventEnvelope,
        _transcript_cursor: rustx::durable::TranscriptCursor,
    ) {
        self.published
            .send_modify(|requests| requests.push(request.clone()));
    }

    fn on_settled(
        &self,
        interaction_id: &InteractionId,
        outcome: &InteractionOutcome,
        _audit: Option<&(RuntimeEventEnvelope, rustx::durable::TranscriptCursor)>,
    ) {
        self.settled
            .lock()
            .expect("settled observations")
            .push((interaction_id.clone(), outcome.clone()));
    }
}

fn interaction_fixture(
    fixture: &common::NativeFixture,
) -> (Arc<InteractionCoordinator>, Arc<QuestionnaireObserver>) {
    let lifecycle = ConversationLifecycle::new();
    assert!(lifecycle.activate());
    let store: Arc<dyn ConversationStore> = fixture.store.clone();
    let coordinator = Arc::new(InteractionCoordinator::new(
        fixture.runtime.conversation_id().clone(),
        lifecycle,
        rustx::durable::interaction_audit_capability(store),
    ));
    let observer = Arc::new(QuestionnaireObserver {
        published: watch::channel(Vec::new()).0,
        settled: Mutex::new(Vec::new()),
    });
    coordinator.set_provider_available(true);
    coordinator.install_observer(observer.clone());
    (coordinator, observer)
}

async fn run(
    fixture: &common::NativeFixture,
    model: &Arc<FakeModel>,
    coordinator: Arc<InteractionCoordinator>,
) -> common::DurableExecutionAudit {
    let capability =
        common::capability_lease(fixture.ordinary_registry.clone(), &fixture.runtime).await;
    let model_snapshot = support::attempt_model(model.clone(), "questionnaire-fixture");
    let context = rustx::context::ContextRuntime::for_attempt(
        rustx::context::SessionContextPolicy {
            reserve_tokens: 0,
            keep_recent_tokens: 0,
            summary_output_cap: None,
        },
        Arc::new(rustx::context::DefaultTokenEstimator),
        Some(rustx::context::AgentStatusEngine::default()),
        &model_snapshot,
        rustx::model::ModelTimeoutPolicy::default(),
        support::default_monotonic_clock(),
    )
    .expect("context runtime");
    let request = AgentExecutionRequest {
        agent_id: AgentId::new("agent-questionnaire"),
        conversation_id: fixture.runtime.conversation_id().clone(),
        attempt_id: AttemptId::new("attempt-questionnaire"),
        conversation: rustx::conversation::ConversationState::from_messages(vec![
            MessageBlock::User(support::fake::inbound_message(
                "message-questionnaire",
                "Ask which homepage style to use.",
                UserSource::Human,
            )),
        ])
        .expect("bootstrap conversation"),
        initial_turn_trigger: rustx::agent::InitialTurnTrigger::Continuation,
        model: model_snapshot,
    };
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let result = AgentExecution::new(
        request,
        capability.into_lease(),
        &cancellation,
        support::default_execution_policy(),
        context,
        &fixture.runtime,
        AttemptLifecycle::inert().with_native_interaction(coordinator),
    )
    .expect("matching conversation identity")
    .run()
    .await;
    common::durable_agent_result(result, fixture.store.as_ref())
}

fn tool_messages(messages: &[MessageBlock]) -> Vec<&ToolMessageBlock> {
    messages
        .iter()
        .filter_map(|message| match message {
            MessageBlock::Tool(tool) => Some(tool),
            _ => None,
        })
        .collect()
}

fn request_tools(request: &ModelRequest) -> Vec<&ToolMessageBlock> {
    request
        .messages
        .iter()
        .filter_map(|message| match message.as_canonical() {
            Some(MessageBlock::Tool(tool)) => Some(tool),
            _ => None,
        })
        .collect()
}

fn assert_call_recorded(messages: &[MessageBlock], expected: &ToolCall) {
    let calls = messages
        .iter()
        .filter_map(|message| match message {
            MessageBlock::Assistant(assistant) => Some(assistant),
            _ => None,
        })
        .flat_map(|assistant| {
            assistant.content.iter().filter_map(|block| match block {
                AssistantContentBlock::ToolCall(call) if call.id == expected.id => Some(call),
                _ => None,
            })
        })
        .collect::<Vec<_>>();
    assert_eq!(calls, [expected], "one exact canonical proposal");
}

async fn answer_published_questionnaire(
    coordinator: &InteractionCoordinator,
    published: &mut watch::Receiver<Vec<InteractionRequest>>,
    expected_call_id: &str,
) -> InteractionRequest {
    let request = published
        .wait_for(|requests| !requests.is_empty())
        .await
        .expect("questionnaire publication channel")[0]
        .clone();
    let InteractionKind::Questionnaire {
        invocation_id,
        questionnaire,
        ..
    } = &request.kind
    else {
        panic!("expected an ordinary questionnaire");
    };
    assert_eq!(
        invocation_id,
        &rustx::tools::types::ToolInvocationId::Agent {
            call_id: ToolCallId::new(expected_call_id),
        }
    );
    assert_eq!(
        questionnaire,
        &expected_questionnaire(),
        "all authored Unicode fields survive publication"
    );
    assert_eq!(coordinator.pending_count(), 1);
    coordinator
        .respond(
            &request.id,
            InteractionResponse::Questionnaire {
                response: QuestionnaireResponse::Submitted(QuestionnaireSubmission {
                    answers: vec![QuestionnaireAnswerEntry {
                        question_index: 0,
                        answer: QuestionnaireAnswer::Option(OptionAnswer { option_index: 1 }),
                    }],
                }),
            },
        )
        .expect("the normal response path accepts the selected option");
    request
}

fn assert_answer(tool: &ToolMessageBlock) {
    assert_eq!(tool.result.status, ToolExecutionStatus::Success);
    assert_eq!(
        tool.result.content,
        vec![ToolResultContent::Json {
            value: json!({
                "cancelled": false,
                "answers": [{
                    "question_index": 0,
                    "question": "您希望首页采用哪一种视觉风格？",
                    "header": "首页风格",
                    "kind": "option",
                    "answer": "生动叙事"
                }]
            }),
        }]
    );
}

#[tokio::test]
async fn unicode_arguments_survive_stream_normalization_history_and_questionnaire_publication() {
    let fixture = common::native_fixture();
    let proposal = call("call-unicode-questionnaire", questionnaire_arguments());
    let PreflightOutcome::Ready(prepared) = fixture
        .registry
        .preflight(&proposal)
        .expect("registered native tool")
    else {
        panic!("the Unicode questionnaire is valid");
    };
    let mut normalized = proposal.arguments.clone();
    normalized["questions"][0]["multi_select"] = json!(false);
    assert_eq!(
        prepared.invocation.arguments, normalized,
        "normalization adds only the default single-select flag"
    );
    let model = fake_model(vec![streamed_turn(&proposal), stop_turn()]);
    let (coordinator, observer) = interaction_fixture(&fixture);
    let mut published = observer.published.subscribe();
    let controller = async {
        answer_published_questionnaire(&coordinator, &mut published, proposal.id.as_str()).await
    };
    let (audit, request) = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        tokio::join!(run(&fixture, &model, coordinator.clone()), controller)
    })
    .await
    .expect("channel-driven questionnaire liveness guard");
    assert_call_recorded(audit.messages(), &proposal);
    let results = tool_messages(audit.messages());
    assert_eq!(results.len(), 1);
    assert_answer(results[0]);
    assert_eq!(model.requests().len(), 2);
    assert_eq!(request_tools(&model.requests()[1]), results);
    assert_eq!(coordinator.pending_count(), 0);
    assert_eq!(observer.published.borrow().len(), 1);
    assert_eq!(observer.settled.lock().unwrap().len(), 1);
    assert!(
        common::read_event_history(
            fixture.store.as_ref(),
            &AttemptId::new("attempt-questionnaire")
        )
        .iter()
        .any(|event| matches!(
            event,
            RuntimeEvent::InteractionRequested {
                interaction_id,
                subject: InteractionSubject::Questionnaire { questionnaire, .. },
            } if *interaction_id == request.id && questionnaire == &expected_questionnaire()
        ))
    );
    support::audit::assert_outcome(
        &audit,
        &AttemptOutcome::Completed {
            finish_reason: ModelFinishReason::Stop,
        },
    );
}

#[tokio::test]
async fn rejected_ask_user_feedback_reaches_next_request_before_scripted_correction() {
    let fixture = common::native_fixture();
    let mut invalid = questionnaire_arguments();
    invalid["questions"][0]["options"][2]["label"] =
        invalid["questions"][0]["options"][0]["label"].clone();
    let rejected = call("call-rejected-questionnaire", invalid);
    let corrected = call("call-corrected-questionnaire", questionnaire_arguments());
    let PreflightOutcome::Rejected { tool_id, error, .. } = fixture
        .registry
        .preflight(&rejected)
        .expect("registered native tool")
    else {
        panic!("duplicate labels must fail native preflight");
    };
    assert_eq!(tool_id, rejected.tool_id);
    assert!(error.contains("questions[0].options[2].label"));
    assert!(error.contains("questions[0].options[0].label"));
    assert!(error.contains("unique"));
    assert!(error.to_lowercase().contains("change"));
    assert!(
        !error.contains("清晰简洁"),
        "diagnostics do not echo authored values"
    );

    let (release, release_rx) = model_release();
    let mut correction_turn = vec![FakeStep::ParkUntilReleased(release_rx)];
    correction_turn.extend(streamed_turn(&corrected));
    let model = fake_model(vec![streamed_turn(&rejected), correction_turn, stop_turn()]);
    let (coordinator, observer) = interaction_fixture(&fixture);
    let mut parked = model.parked();
    let mut published = observer.published.subscribe();
    let controller = async {
        parked
            .wait_for(|parked| *parked)
            .await
            .expect("second model turn parks before proposing its correction");
        let requests = model.requests();
        assert_eq!(
            requests.len(),
            2,
            "the rejection causes one ordinary continuation"
        );
        let tools = request_tools(&requests[1]);
        assert_eq!(
            tools.len(),
            1,
            "one failed result in the exact next request"
        );
        let failed = tools[0];
        assert_eq!(failed.tool_call_id, rejected.id);
        assert_eq!(failed.tool_id, rejected.tool_id);
        assert_eq!(
            failed.result.status,
            ToolExecutionStatus::Failed {
                error: error.clone()
            }
        );
        assert!(
            failed.result.content.is_empty(),
            "status remains separate from tool-owned content"
        );
        assert_eq!(
            failed.result.model_facing_projection().as_text(),
            format!("Tool call failed: {error}")
        );
        assert!(
            failed.result.model_facing_projection().byte_len()
                <= rustx::tools::limits::MAX_MODEL_TOOL_RESULT_BYTES
        );
        assert_eq!(coordinator.pending_count(), 0);
        assert!(
            observer.published.borrow().is_empty(),
            "rejected native arguments cannot publish a questionnaire"
        );
        let events = common::read_event_history(
            fixture.store.as_ref(),
            &AttemptId::new("attempt-questionnaire"),
        );
        assert!(!events.iter().any(|event| matches!(
            event,
            RuntimeEvent::ToolExecutionStarted { .. }
                | RuntimeEvent::ToolExecutionCompleted { .. }
                | RuntimeEvent::InteractionRequested { .. }
        )));
        let committed = events
            .iter()
            .enumerate()
            .filter_map(|(index, event)| match event {
                RuntimeEvent::ToolMessageCommitted { tool_call_id, .. }
                    if *tool_call_id == rejected.id =>
                {
                    Some(index)
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(committed.len(), 1);
        let assistant_commit = events
            .iter()
            .position(|event| {
                matches!(
                    event, RuntimeEvent::AssistantMessageCommitted { message_id }
                        if *message_id == failed.occurrence.assistant_message_id
                )
            })
            .expect("the failed result retains its issuing assistant occurrence");
        let next_request = events
            .iter()
            .enumerate()
            .filter_map(|(index, event)| {
                matches!(event, RuntimeEvent::ModelRequestStarted { .. }).then_some(index)
            })
            .nth(1)
            .expect("the exact next request started");
        assert!(assistant_commit < committed[0]);
        assert!(
            committed[0] < next_request,
            "the failed result is durable before the next request starts"
        );
        release.send_replace(true);
        let request =
            answer_published_questionnaire(&coordinator, &mut published, corrected.id.as_str())
                .await;
        (failed.clone(), request)
    };
    let (audit, (failed, questionnaire)) =
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            tokio::join!(run(&fixture, &model, coordinator.clone()), controller)
        })
        .await
        .expect("channel-driven correction liveness guard");

    assert_call_recorded(audit.messages(), &rejected);
    assert_call_recorded(audit.messages(), &corrected);
    let results = tool_messages(audit.messages());
    assert_eq!(
        results.len(),
        2,
        "one canonical slot for each scripted proposal"
    );
    assert_eq!(results[0], &failed);
    assert_eq!(results[1].tool_call_id, corrected.id);
    assert_answer(results[1]);
    let requests = model.requests();
    assert_eq!(
        requests.len(),
        3,
        "two scripted tool proposals followed by the final answer"
    );
    assert_eq!(
        request_tools(&requests[2]),
        results,
        "the following request sees both results once and in call order"
    );
    assert_eq!(coordinator.pending_count(), 0);
    assert_eq!(observer.published.borrow().len(), 1);
    assert_eq!(observer.settled.lock().unwrap().len(), 1);
    let events = common::read_event_history(
        fixture.store.as_ref(),
        &AttemptId::new("attempt-questionnaire"),
    );
    let starts = events
        .iter()
        .filter_map(|event| match event {
            RuntimeEvent::ToolExecutionStarted { tool_call_id, .. } => Some(tool_call_id),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        starts,
        [&corrected.id],
        "only the corrected call enters the native executor"
    );
    let completions = events
        .iter()
        .filter_map(|event| match event {
            RuntimeEvent::ToolExecutionCompleted { tool_call_id, .. } => Some(tool_call_id),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        completions,
        [&corrected.id],
        "the rejected call never gains execution lifecycle facts"
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, RuntimeEvent::ModelRetryScheduled { .. }))
    );
    let result_commits = events
        .iter()
        .filter_map(|event| match event {
            RuntimeEvent::ToolMessageCommitted { tool_call_id, .. } => Some(tool_call_id),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(result_commits, [&rejected.id, &corrected.id]);
    let settled_index = events
        .iter()
        .position(|event| {
            matches!(event,
                RuntimeEvent::InteractionSettled {
                    interaction_id,
                    settlement: InteractionSettlement::QuestionnaireSubmitted { .. },
                } if *interaction_id == questionnaire.id
            )
        })
        .expect("the answer commits an interaction settlement");
    let result_index = events.iter().position(|event| matches!(event,
        RuntimeEvent::ToolMessageCommitted { tool_call_id, .. } if *tool_call_id == corrected.id
    )).expect("the corrected result commits");
    assert!(
        settled_index < result_index,
        "the response settles before the canonical result"
    );
    support::audit::assert_outcome(
        &audit,
        &AttemptOutcome::Completed {
            finish_reason: ModelFinishReason::Stop,
        },
    );
}
