//! Native human waits: channel publication, manual deadlines, exact typed responses.
use super::*;
use crate::events::review::{ReviewDecision, ReviewResponse};
use crate::runtime::interaction::*;

struct Published(tokio::sync::mpsc::UnboundedSender<InteractionRequest>);
impl InteractionObserver for Published {
    fn on_pending(
        &self,
        request: &InteractionRequest,
        _: &RuntimeEventEnvelope,
        _: crate::durable::TranscriptCursor,
    ) {
        self.0.send(request.clone()).unwrap();
    }
    fn on_settled(
        &self,
        _: &crate::runtime::identity::InteractionId,
        _: &InteractionOutcome,
        _: Option<&(RuntimeEventEnvelope, crate::durable::TranscriptCursor)>,
    ) {
    }
}
pub(super) fn owner(
    plane: &WorkflowTestPlane,
) -> (
    Arc<InteractionCoordinator>,
    Arc<RecordingInteractionAudit>,
    tokio::sync::mpsc::UnboundedReceiver<InteractionRequest>,
) {
    let lifecycle = crate::runtime::types::ConversationLifecycle::new();
    assert!(lifecycle.activate());
    let audit = RecordingInteractionAudit::new(plane.conversation_id.clone());
    let owner = Arc::new(InteractionCoordinator::new(
        plane.conversation_id.clone(),
        lifecycle,
        audit.clone(),
    ));
    owner.set_provider_available(true);
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    owner.install_observer(Arc::new(Published(tx)));
    (owner, audit, rx)
}
fn human_program(question: bool) -> Arc<WorkflowProgram> {
    Arc::new(compile_test(human_definition(question)).unwrap())
}
fn human_definition(question: bool) -> WorkflowDefinition {
    let result = if question {
        schema(
            json!({"cancelled":{"type":"boolean"},"answers":{"type":"array","items":{"type":"object"}}}),
            &["cancelled", "answers"],
        )
    } else {
        crate::runtime::workflow::review::result_schema()
    };
    let node = if question {
        json!({"type":"tool","selector":{"origin":"builtin","name":"ask_user"},"arguments":{"type":"literal","value":{"questions":[{"question":"Choose target","header":"Target","options":[{"label":"A","description":"First"},{"label":"B","description":"Second"}]}]}},"result":{"type":"json","part":0,"schema":result}})
    } else {
        json!({"type":"review","subject":{"type":"plan","value":{"type":"reference","path":["args"]}},"context":[]})
    };
    serde_json::from_value(json!({"description":"Human business decision","timeout_ms":100,"tools":if question {json!([{"origin":"builtin","name":"ask_user"}])} else {json!([])},"block":{
        "input":schema(json!({"passed":{"type":"boolean"}}), &["passed"]),"output":result,"entry":"human",
        "nodes":{"human":node,"branch":{"type":"branch","condition":{"type":"boolean","value":{"type":"reference","path":["human",if question {"cancelled"} else {"accepted"}]}}},"yes":{"type":"return","output":{"type":"reference","path":["human"]}},"no":{"type":"return","output":{"type":"reference","path":["human"]}}},
        "edges":[{"from":"human","to":"branch"},{"from":"branch","to":"yes","port":"true"},{"from":"branch","to":"no","port":"false"}]}})).unwrap()
}
fn human_context(
    plane: &WorkflowTestPlane,
    owner: Arc<InteractionCoordinator>,
) -> crate::runtime::subagent::AttemptSubagentContext {
    context_with_registration(
        plane,
        crate::tools::native::test_ask_user_registration(),
        crate::agent::AttemptLifecycle::default()
            .with_pre_tool_policy(Arc::new(
                crate::agent::lifecycle::ConfiguredApprovalPolicy::new(ApprovalMode::FullAccess),
            ))
            .with_native_interaction(owner),
    )
}

fn feedback_definition(question: bool) -> WorkflowDefinition {
    let mut definition = super::loops::wrap_definition(human_definition(question), 3);
    let WorkflowNodeDefinition::Loop { until, carry, .. } =
        definition.block.nodes.get_mut("feedback").unwrap()
    else {
        unreachable!()
    };
    let boolean = WorkflowPredicate::Boolean {
        value: reference(if question {
            "result.cancelled"
        } else {
            "result.accepted"
        }),
    };
    **until = if question {
        WorkflowPredicate::Not {
            predicate: Box::new(boolean),
        }
    } else {
        boolean
    };
    *carry = WorkflowValue::Literal {
        value: json!({"passed":false}),
    };
    definition
}

#[tokio::test]
async fn loop_review_and_questionnaire_reject_old_responses_and_allocate_fresh_instances() {
    for question in [false, true] {
        let plane = workflow_test_plane(1);
        let (owner, audit, mut published) = owner(&plane);
        let context = human_context(&plane, owner.clone());
        let runtime = workflow_runtime(&plane);
        let (_, cancellation) = workflow_cancellation();
        let definition = feedback_definition(question);
        let task = tokio::spawn(async move {
            runtime
                .run_foreground(
                    Arc::new(compile_test(definition).unwrap()),
                    ToolCallId::new("loop-human"),
                    context,
                    json!({"passed":false}),
                    cancellation,
                )
                .await
        });
        let first = published.recv().await.unwrap();
        owner
            .respond_async(&first.id, answer(&first, false))
            .await
            .unwrap();
        let second = published.recv().await.unwrap();
        assert_ne!(first.id, second.id);
        assert!(
            owner
                .respond_async(&first.id, answer(&first, true))
                .await
                .is_err()
        );
        if let (
            InteractionKind::Review { review: a, .. },
            InteractionKind::Review { review: b, .. },
        ) = (&first.kind, &second.kind)
        {
            assert_eq!(a.instance.block.definition, b.instance.block.definition);
            assert_eq!(a.instance.block.invocations, [0, 1]);
            assert_eq!(b.instance.block.invocations, [0, 2]);
            assert!(
                owner
                    .respond_async(&second.id, answer(&first, true))
                    .await
                    .is_err()
            );
        }
        if let (
            InteractionKind::Questionnaire {
                invocation_id: a, ..
            },
            InteractionKind::Questionnaire {
                invocation_id: b, ..
            },
        ) = (&first.kind, &second.kind)
        {
            assert_ne!(a, b);
        }
        assert!(!task.is_finished());
        owner
            .respond_async(&second.id, answer(&second, true))
            .await
            .unwrap();
        let output = task.await.unwrap().unwrap();
        assert_eq!(output["status"], "satisfied");
        assert_eq!(output["iterations"], 2);
        assert_eq!(audit.events().len(), 4);
        assert!(owner.pending_snapshot().is_empty());
        assert!(plane.registry.all_snapshots().is_empty());
    }
}

#[tokio::test]
async fn unavailable_loop_human_step_fails_without_exhaustion_or_reprompt() {
    let plane = workflow_test_plane(1);
    let (owner, _, mut published) = owner(&plane);
    owner.set_provider_available(false);
    let context = human_context(&plane, owner);
    let runtime = workflow_runtime(&plane);
    let observations = runtime.observations.subscribe();
    let (_, cancellation) = workflow_cancellation();
    assert!(
        runtime
            .run_foreground(
                Arc::new(compile_test(feedback_definition(false)).unwrap()),
                ToolCallId::new("unavailable-loop"),
                context,
                json!({"passed":false}),
                cancellation
            )
            .await
            .is_err()
    );
    assert!(published.try_recv().is_err());
    assert_eq!(
        observations
            .borrow()
            .iter()
            .filter(|e| matches!(e, RuntimeEvent::WorkflowLoopIterationAdmitted { .. }))
            .count(),
        1
    );
    assert!(
        !observations
            .borrow()
            .iter()
            .any(|e| matches!(e, RuntimeEvent::WorkflowLoopExited { .. }))
    );
}
pub(super) fn answer(request: &InteractionRequest, affirmative: bool) -> InteractionResponse {
    match &request.kind {
        InteractionKind::Review {
            review,
            subject_digest,
        } => InteractionResponse::Review {
            response: ReviewResponse {
                instance: review.instance.clone(),
                subject_digest: subject_digest.clone(),
                decision: if affirmative {
                    ReviewDecision::Accepted
                } else {
                    ReviewDecision::Rejected {
                        feedback: "Revise scope".into(),
                    }
                },
            },
        },
        InteractionKind::Questionnaire { .. } => InteractionResponse::Questionnaire {
            response: if affirmative {
                QuestionnaireResponse::Submitted(QuestionnaireSubmission {
                    answers: vec![QuestionnaireAnswerEntry {
                        question_index: 0,
                        answer: QuestionnaireAnswer::SingleOption(SingleOptionAnswer {
                            label: "A".into(),
                        }),
                    }],
                })
            } else {
                QuestionnaireResponse::Declined
            },
        },
        InteractionKind::Approval { .. } => {
            panic!("FullAccess did not bypass configured Tool Approval")
        }
    }
}
#[tokio::test]
async fn fixed_native_questions_and_plan_review_branch_without_model_or_history_under_full_access()
{
    for question in [false, true] {
        for affirmative in [false, true] {
            let plane = workflow_test_plane(1);
            let (owner, audit, mut published) = owner(&plane);
            let context = human_context(&plane, owner.clone());
            let runtime = workflow_runtime(&plane);
            let (_, cancellation) = workflow_cancellation();
            let task = tokio::spawn(async move {
                runtime
                    .run_foreground(
                        human_program(question),
                        ToolCallId::new("outer"),
                        context,
                        json!({"passed":false}),
                        cancellation,
                    )
                    .await
            });
            let request = published.recv().await.unwrap();
            assert!(!task.is_finished());
            assert_eq!(audit.events().len(), 1); // durable before prompt callback
            if let InteractionKind::Questionnaire { invocation_id, .. } = &request.kind {
                assert!(matches!(invocation_id, ToolInvocationId::Workflow { .. }));
            }
            if let InteractionKind::Review { review, .. } = &request.kind {
                assert_eq!(
                    review.instance.block.run.conversation_id,
                    plane.conversation_id
                );
            }
            let response = answer(&request, affirmative);
            owner
                .respond_async(&request.id, response.clone())
                .await
                .unwrap();
            assert!(owner.respond_async(&request.id, response).await.is_err());
            let value = task.await.unwrap().unwrap();
            assert_eq!(audit.events().len(), 2);
            let expected = if question { !affirmative } else { affirmative };
            assert_eq!(
                value[if question { "cancelled" } else { "accepted" }],
                expected
            );
            if !question && !affirmative {
                assert_eq!(value["feedback"], "Revise scope");
            }
            let events = plane.store.read_events(None, 128).unwrap().events;
            assert!(events.iter().any(|event| matches!(&event.event, RuntimeEvent::WorkflowBranchSelected { successor, .. } if successor == if expected { "yes" } else { "no" })));
            assert!(!events.iter().any(|event| matches!(
                event.event,
                RuntimeEvent::ModelRequestStarted { .. }
                    | RuntimeEvent::AssistantMessageCommitted { .. }
                    | RuntimeEvent::ToolMessageCommitted { .. }
            )));
        }
    }
}
#[tokio::test]
async fn human_wait_uses_outer_manual_deadline_and_never_synthesizes_business_response() {
    for question in [false, true] {
        let plane = workflow_test_plane(1);
        let (owner, audit, mut published) = owner(&plane);
        let mut context = human_context(&plane, owner.clone());
        let clock = Arc::new(crate::runtime::ManualMonotonicClock::new());
        Arc::make_mut(context.native.as_mut().unwrap()).clock = clock.clone();
        let (_, cancellation) = workflow_cancellation();
        let task = tokio::spawn(run_outer(
            workflow_runtime(&plane),
            human_program(question),
            context,
            cancellation,
        ));
        let request = published.recv().await.unwrap();
        clock.advance(100);
        let (result, _) = task.await.unwrap();
        assert_eq!(result.status, ToolExecutionStatus::TimedOut);
        assert_eq!(owner.pending_count(), 0);
        assert!(matches!(
            audit.events().last(),
            Some(RuntimeEvent::InteractionSettled {
                settlement: crate::events::interaction::InteractionSettlement::DeadlineExpired { .. },
                ..
            })
        ));
        assert!(
            owner
                .respond_async(&request.id, answer(&request, true))
                .await
                .is_err()
        );
        assert!(!plane.store.read_events(None, 128).unwrap().events.iter().any(|event| matches!(&event.event, RuntimeEvent::WorkflowNodeStarted { instance } if instance.node != "human")));
    }
}
#[tokio::test]
async fn review_wrong_instance_subject_and_kind_leave_original_pending_detach_resync_is_live_only()
{
    let plane = workflow_test_plane(1);
    let (owner, audit, mut published) = owner(&plane);
    let context = human_context(&plane, owner.clone());
    let (_, cancellation) = workflow_cancellation();
    let runtime = workflow_runtime(&plane);
    let task = tokio::spawn(async move {
        runtime
            .run_foreground(
                human_program(false),
                ToolCallId::new("outer"),
                context,
                json!({"passed":false}),
                cancellation,
            )
            .await
    });
    let request = published.recv().await.unwrap();
    for field in 0..5 {
        let InteractionResponse::Review { mut response } = answer(&request, true) else {
            unreachable!()
        };
        match field {
            0 => response.instance.visit += 1,
            1 => response.instance.block.run.invocation += 1,
            2 => response.instance.block.invocations.push(1),
            3 => response.instance.node = "other".into(),
            _ => response.subject_digest = "0".repeat(64),
        }
        assert!(
            owner
                .respond_async(&request.id, InteractionResponse::Review { response })
                .await
                .is_err()
        );
    }
    assert!(
        owner
            .respond_async(
                &request.id,
                InteractionResponse::Questionnaire {
                    response: QuestionnaireResponse::Declined
                }
            )
            .await
            .is_err()
    );
    owner.set_provider_available(false);
    assert_eq!(owner.pending_snapshot(), vec![request.clone()]);
    owner.set_provider_available(true);
    assert_eq!(audit.events().len(), 1);
    owner
        .respond_async(&request.id, answer(&request, true))
        .await
        .unwrap();
    task.await.unwrap().unwrap();
}

#[tokio::test]
async fn review_provider_absence_audit_failures_and_cancel_before_response_are_not_rejection() {
    for mode in 0..4 {
        let plane = workflow_test_plane(1);
        let (owner, audit, mut published) = owner(&plane);
        if mode == 0 {
            owner.set_provider_available(false);
        }
        if mode == 1 {
            audit.fail_next_requested();
        }
        let context = human_context(&plane, owner.clone());
        let (trigger, cancellation) = workflow_cancellation();
        let runtime = workflow_runtime(&plane);
        let task = tokio::spawn(async move {
            runtime
                .run_foreground(
                    human_program(false),
                    ToolCallId::new("outer"),
                    context,
                    json!({"passed":false}),
                    cancellation,
                )
                .await
        });
        if mode >= 2 {
            let request = published.recv().await.unwrap();
            if mode == 2 {
                audit.fail_next_settled();
            } else {
                trigger.cancel();
            }
            assert!(
                owner
                    .respond_async(&request.id, answer(&request, true))
                    .await
                    .is_err()
            );
        }
        assert!(task.await.unwrap().is_err());
        assert_eq!(owner.pending_count(), 0);
        assert!(!audit.events().iter().any(|event| matches!(
            event,
            RuntimeEvent::InteractionSettled {
                settlement: crate::events::interaction::InteractionSettlement::Reviewed { .. },
                ..
            }
        )));
        assert!(!plane.store.read_events(None, 128).unwrap().events.iter().any(|event| matches!(&event.event, RuntimeEvent::WorkflowNodeStarted { instance } if instance.node != "human")));
    }
}

#[tokio::test]
async fn simultaneous_workflow_review_and_questionnaire_settle_only_their_original_identity() {
    let plane = workflow_test_plane(1);
    let (owner, audit, mut published) = owner(&plane);
    let context = human_context(&plane, owner.clone());
    let runtime = Arc::new(workflow_runtime(&plane));
    let mut tasks = Vec::new();
    for question in [false, true] {
        let context = context.clone();
        let runtime = runtime.clone();
        let (_, cancellation) = workflow_cancellation();
        tasks.push(tokio::spawn(async move {
            runtime
                .run_foreground(
                    human_program(question),
                    ToolCallId::new("outer"),
                    context,
                    json!({"passed":false}),
                    cancellation,
                )
                .await
        }));
    }
    let first = published.recv().await.unwrap();
    let second = published.recv().await.unwrap();
    assert_ne!(first.interaction_ref(), second.interaction_ref());
    assert_eq!(owner.pending_count(), 2);
    owner
        .respond_async(&second.id, answer(&second, true))
        .await
        .unwrap();
    assert_eq!(owner.pending_snapshot(), vec![first.clone()]);
    assert!(
        owner
            .respond_async(&first.id, answer(&second, true))
            .await
            .is_err()
    );
    owner
        .respond_async(&first.id, answer(&first, true))
        .await
        .unwrap();
    for task in tasks {
        task.await.unwrap().unwrap();
    }
    assert_eq!(audit.events().len(), 4);
}

#[test]
fn documented_fixed_question_and_review_example_compiles_without_profiles() {
    let definition = serde_yaml::from_str(include_str!(
        "../../../../examples/local-runtime/workspace/.agents/workflows/human_review.yaml"
    ))
    .unwrap();
    WorkflowProgram::compile(
        WorkflowId::parse("human_review").unwrap(),
        definition,
        &BTreeSet::new(),
    )
    .unwrap();
}
