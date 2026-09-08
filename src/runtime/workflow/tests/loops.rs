//! WF-05: fixed bodies, exact admission gates, native settlement and typed exits.
use super::*;

#[test]
fn documented_bounded_review_uses_the_current_grammar() {
    let definition = serde_yaml::from_str(include_str!(
        "../../../../examples/local-runtime/workspace/.agents/workflows/bounded_review.yaml"
    ))
    .unwrap();
    compile_test(definition).unwrap();
}

fn agent_feedback_definition(max: u32) -> WorkflowDefinition {
    let state = schema(json!({"passed":{"type":"boolean"}}), &["passed"]);
    let definition = serde_json::from_value(json!({"description":"native child feedback","block":{
        "input":state,"output":state,"entry":"work","nodes":{
            "work":{"type":"agent","profile":"reviewer","task":"Check fixed input","input":{},"output":state},
            "done":{"type":"return","output":{"type":"reference","path":["work"]}}
        },"edges":[{"from":"work","to":"done"}]}})).unwrap();
    wrap_definition(definition, max)
}

#[tokio::test]
async fn interrupted_child_and_child_deadline_preserve_native_certainty_without_retry() {
    use crate::runtime::subagent::ipc::ChildResultStatus;
    for mode in [
        "interrupted",
        "deadline",
        "deadline_then_parent_cancel",
        "failed",
    ] {
        let plane = workflow_test_plane(1);
        let mut child = stage_workflow_child(&plane);
        let runtime = workflow_runtime(&plane);
        let observations = runtime.observations.subscribe();
        let context = context(
            &plane,
            Probe::new(ToolExecutionStatus::Success),
            crate::agent::AttemptLifecycle::default(),
        );
        let (trigger, cancellation) = workflow_cancellation();
        let task = tokio::spawn(run_outer(
            runtime,
            Arc::new(compile_test(agent_feedback_definition(3)).unwrap()),
            context,
            cancellation,
        ));
        child.expect_delegate().await;
        if mode.starts_with("deadline") {
            let id = plane.registry.all_snapshots()[0].subagent_id.clone();
            plane
                .registry
                .cancel(&id, CancellationReason::SubagentExecutionDeadlineExceeded)
                .unwrap();
            if mode == "deadline_then_parent_cancel" {
                trigger.cancel();
            }
            let frame = crate::runtime::subagent::ipc::read_parent_frame(&mut child.peer)
                .await
                .unwrap();
            assert!(matches!(
                frame,
                Some(crate::runtime::subagent::ipc::ParentFrame::Cancel {
                    reason: Some(CancellationReason::SubagentExecutionDeadlineExceeded)
                })
            ));
            child.send_result(ChildResultStatus::Cancelled, None).await;
        } else if mode == "failed" {
            child.send_result(ChildResultStatus::Failed, None).await;
        } else {
            drop(child);
        }
        let (result, _) = task.await.unwrap();
        match mode {
            "deadline" | "deadline_then_parent_cancel" => {
                assert_eq!(result.status, ToolExecutionStatus::TimedOut);
            }
            "failed" => assert!(matches!(result.status, ToolExecutionStatus::Failed { .. })),
            _ => assert!(matches!(
                result.status,
                ToolExecutionStatus::OutcomeUnknown { .. }
            )),
        }
        assert_eq!(plane.registry.all_snapshots().len(), 1);
        assert!(plane.registry.unsettled_snapshot().is_empty());
        assert!(
            !observations
                .borrow()
                .iter()
                .any(|e| matches!(e, RuntimeEvent::WorkflowLoopExited { .. }))
        );
    }
}

#[test]
fn aggregate_agent_expansion_is_separate_from_static_nodes_and_steps() {
    assert!(compile_test(agent_feedback_definition(256)).is_ok());
    let mut definition = agent_feedback_definition(256);
    let WorkflowNodeDefinition::Loop { body, .. } = &definition.block.nodes["feedback"] else {
        unreachable!()
    };
    definition
        .block
        .nodes
        .insert("before".into(), body.nodes["work"].clone());
    definition.block.entry = "before".into();
    definition.block.edges.push(edge("before", "feedback"));
    assert!(
        matches!(compile_test(definition),Err(WorkflowCompileError::InvalidField(message)) if message.contains("expanded execution"))
    );
}

#[tokio::test]
async fn oversized_carry_commits_no_partial_state_and_admits_no_next_body() {
    let plane = workflow_test_plane(1);
    let runtime = workflow_runtime(&plane);
    let observations = runtime.observations.subscribe();
    let context = workflow_test_context(&plane);
    let state = schema(
        json!({"passed":{"type":"boolean"},"a":{"type":"string"},"b":{"type":"string"}}),
        &["passed", "a", "b"],
    );
    let definition = serde_json::from_value(json!({"description":"bounded carry","block":{
        "input":state,"output":state,"entry":"done","nodes":{"done":{"type":"return","output":{"type":"reference","path":["args"]}}},"edges":[]}})).unwrap();
    let mut definition = wrap_definition(definition, 3);
    let WorkflowNodeDefinition::Loop { carry, .. } =
        definition.block.nodes.get_mut("feedback").unwrap()
    else {
        unreachable!()
    };
    *carry = WorkflowValue::Object {
        fields: BTreeMap::from([
            ("passed".into(), reference("result.passed")),
            ("a".into(), reference("result.a")),
            ("b".into(), reference("result.a")),
        ]),
    };
    let (_, cancellation) = workflow_cancellation();
    let error = runtime
        .run_foreground(
            Arc::new(compile_test(definition).unwrap()),
            ToolCallId::new("oversized-carry"),
            context,
            json!({"passed":false,"a":"x".repeat(MAX_VALUE_BYTES/2),"b":""}),
            cancellation,
        )
        .await
        .unwrap_err();
    assert!(matches!(error, WorkflowRunError::InvalidValue(_)));
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
    assert!(plane.registry.all_snapshots().is_empty());
}

pub(super) fn wrap_definition(
    mut definition: WorkflowDefinition,
    max_iterations: u32,
) -> WorkflowDefinition {
    let body = definition.block;
    let output = execution::loop_result_schema(&body.output);
    definition.block = WorkflowBlock {
        input: body.input.clone(),
        output,
        entry: "feedback".into(),
        nodes: BTreeMap::from([
            (
                "feedback".into(),
                WorkflowNodeDefinition::Loop {
                    input: reference("args"),
                    body: Box::new(body),
                    until: Box::new(WorkflowPredicate::Boolean {
                        value: reference("result.passed"),
                    }),
                    carry: reference("result"),
                    max_iterations,
                },
            ),
            (
                "done".into(),
                WorkflowNodeDefinition::Return {
                    output: reference("feedback"),
                },
            ),
        ]),
        edges: vec![edge("feedback", "done")],
    };
    definition
}

struct Checker {
    starts: AtomicUsize,
    satisfied_at: usize,
    received: std::sync::Mutex<Vec<ToolInvocation>>,
}
impl ToolExecutor for Checker {
    fn progress_capability(&self) -> crate::tools::deadline::ToolProgressCapability {
        crate::tools::deadline::ToolProgressCapability::None
    }
    fn start<'a>(
        &'a self,
        invocation: ToolInvocation,
        context: ToolExecutionContext<'a>,
    ) -> ToolExecutionHandle<'a> {
        ToolExecutionHandle::settled_by_operation(
            Box::pin(async move {
                self.received.lock().unwrap().push(invocation);
                let count = self.starts.fetch_add(1, Ordering::SeqCst) + 1;
                let mut result = terminal(ToolExecutionStatus::Success);
                result.content = vec![ToolResultContent::Json {
                    value: json!({"passed":count >= self.satisfied_at, "label":format!("committed-{count}")}),
                }];
                result
            }),
            context.cancellation,
        )
    }
}

fn checker_definition(max: u32) -> WorkflowDefinition {
    let state = schema(
        json!({"passed":{"type":"boolean"},"label":{"type":"string"}}),
        &["passed", "label"],
    );
    let body = serde_json::from_value(json!({"description":"feedback","tools":[{"origin":"builtin","name":"check"}],"block":{
        "input":state,"output":state,"entry":"check","nodes":{
            "check":{"type":"tool","selector":{"origin":"builtin","name":"check"},"arguments":{"type":"reference","path":["args"]},"result":{"type":"json","part":0,"schema":state}},
            "done":{"type":"return","output":{"type":"reference","path":["check"]}}
        },"edges":[{"from":"check","to":"done"}]}})).unwrap();
    wrap_definition(body, max)
}

#[tokio::test]
async fn exact_satisfaction_exhaustion_carry_and_concrete_tool_identity() {
    for (satisfied_at, max, count, status) in [
        (1, 4, 1, "satisfied"),
        (3, 4, 3, "satisfied"),
        (9, 4, 4, "exhausted"),
    ] {
        let plane = workflow_test_plane(1);
        let runtime = workflow_runtime(&plane);
        let observations = runtime.observations.subscribe();
        let checker = Arc::new(Checker {
            starts: AtomicUsize::new(0),
            satisfied_at,
            received: std::sync::Mutex::new(Vec::new()),
        });
        let context = context_with_registration(
            &plane,
            ToolRegistration::plain(definition(), checker.clone()),
            crate::agent::AttemptLifecycle::default(),
        );
        let (_, cancellation) = workflow_cancellation();
        let output = runtime
            .run_foreground(
                Arc::new(compile_test(checker_definition(max)).unwrap()),
                ToolCallId::new("loop"),
                context,
                json!({"passed":false,"label":"initial"}),
                cancellation,
            )
            .await
            .unwrap();
        assert_eq!(
            output,
            json!({"status":status,"iterations":count,"result":{"passed":status == "satisfied","label":format!("committed-{count}")}})
        );
        assert_eq!(checker.starts.load(Ordering::SeqCst), count);
        let received = checker.received.lock().unwrap();
        for (index, call) in received.iter().enumerate() {
            assert_eq!(
                call.arguments["label"],
                if index == 0 {
                    "initial".into()
                } else {
                    format!("committed-{index}")
                }
            );
            let ToolInvocationId::Workflow { node } = &call.id else {
                panic!("native workflow identity")
            };
            assert_eq!(node.block.definition.blocks, ["feedback", "body"]);
            assert_eq!(
                node.block.invocations,
                [0, u32::try_from(index + 1).unwrap()]
            );
            if index > 0 {
                assert_ne!(call.id, received[index - 1].id);
            }
        }
        let events = observations.borrow();
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(e, RuntimeEvent::WorkflowLoopIterationAdmitted { .. }))
                .count(),
            count
        );
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(
                    e,
                    RuntimeEvent::WorkflowLoopIterationSettled {
                        outcome: WorkflowExecutionOutcome::Completed,
                        ..
                    }
                ))
                .count(),
            count
        );
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(e, RuntimeEvent::WorkflowLoopExited { .. }))
                .count(),
            1
        );
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(e, RuntimeEvent::WorkflowCompleted { .. }))
                .count(),
            1
        );
        assert!(plane.registry.all_snapshots().is_empty());
    }
}

#[tokio::test]
async fn cancellation_at_second_iteration_frontier_consumes_and_starts_nothing_new() {
    let plane = workflow_test_plane(1);
    let runtime = workflow_runtime(&plane);
    let observations = runtime.observations.subscribe();
    let probe = Probe::new(ToolExecutionStatus::Success);
    let context = context(
        &plane,
        probe.clone(),
        crate::agent::AttemptLifecycle::default(),
    );
    let (entered, enter) = tokio::sync::oneshot::channel();
    let (release, wait) = tokio::sync::oneshot::channel();
    *runtime.iteration_frontier.lock().unwrap() = Some(execution::IterationFrontierHook {
        iteration: 2,
        entered,
        release: wait,
    });
    let (trigger, cancellation) = workflow_cancellation();
    let program = Arc::new(compile_test(wrap_definition(program_definition(), 3)).unwrap());
    let task = tokio::spawn(async move {
        runtime
            .run_foreground(
                program,
                ToolCallId::new("cancel-loop"),
                context,
                json!({"passed":false}),
                cancellation,
            )
            .await
    });
    enter.await.unwrap();
    assert_eq!(probe.starts.load(Ordering::SeqCst), 1);
    trigger.cancel();
    release.send(()).unwrap();
    assert!(task.await.unwrap().unwrap_err().is_cancelled());
    assert_eq!(probe.starts.load(Ordering::SeqCst), 1);
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

#[tokio::test]
async fn every_native_non_success_remains_outer_status_without_a_second_iteration() {
    for status in [
        ToolExecutionStatus::Failed {
            error: "failed".into(),
        },
        ToolExecutionStatus::Denied {
            reason: "denied".into(),
        },
        ToolExecutionStatus::TimedOut,
        ToolExecutionStatus::Cancelled {
            reason: CancellationReason::UserRequested,
            phase: ToolCancellationPhase::DuringExecution,
        },
        ToolExecutionStatus::OutcomeUnknown {
            detail: "unknown".into(),
        },
    ] {
        let plane = workflow_test_plane(1);
        let runtime = workflow_runtime(&plane);
        let observations = runtime.observations.subscribe();
        let probe = Probe::new(status.clone());
        let context = context(
            &plane,
            probe.clone(),
            crate::agent::AttemptLifecycle::default(),
        );
        let (_, cancellation) = workflow_cancellation();
        let (result, _) = run_outer(
            runtime,
            Arc::new(compile_test(wrap_definition(program_definition(), 3)).unwrap()),
            context,
            cancellation,
        )
        .await;
        assert_eq!(result.status, status);
        assert!(result.content.is_empty());
        assert_eq!(probe.starts.load(Ordering::SeqCst), 1);
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
}

#[test]
fn invalid_limits_carry_predicates_and_back_edges_are_rejected() {
    for max in [0, MAX_LOOP_ITERATIONS + 1, u32::MAX] {
        assert!(compile_test(checker_definition(max)).is_err());
    }
    for expression in [
        reference("args"),
        reference("check"),
        WorkflowValue::Literal {
            value: json!({"passed":"false"}),
        },
    ] {
        let mut definition = checker_definition(2);
        let WorkflowNodeDefinition::Loop { carry, .. } =
            definition.block.nodes.get_mut("feedback").unwrap()
        else {
            unreachable!()
        };
        *carry = expression;
        assert!(compile_test(definition).is_err());
    }
    let mut definition = checker_definition(2);
    let WorkflowNodeDefinition::Loop { body, .. } =
        definition.block.nodes.get_mut("feedback").unwrap()
    else {
        unreachable!()
    };
    body.edges.push(edge("done", "check"));
    assert!(compile_test(definition).is_err());
}

#[test]
fn nested_execution_expansion_is_checked_and_private_memory_is_not_multiplied() {
    let once = compile_test(checker_definition(1)).unwrap();
    let repeated = compile_test(checker_definition(MAX_LOOP_ITERATIONS)).unwrap();
    assert_eq!(once.retained_bound, repeated.retained_bound);
    assert!(repeated.execution_bound > once.execution_bound);
    let mut definition = checker_definition(100);
    let WorkflowNodeDefinition::Loop { body, .. } =
        definition.block.nodes.get_mut("feedback").unwrap()
    else {
        unreachable!()
    };
    **body = checker_definition(100).block;
    // Fix the enclosing contracts to expose expanded execution, not a type error.
    let result = body.output.clone();
    let WorkflowNodeDefinition::Loop { until, carry, .. } =
        definition.block.nodes.get_mut("feedback").unwrap()
    else {
        unreachable!()
    };
    **until = WorkflowPredicate::Boolean {
        value: reference("result.result.passed"),
    };
    *carry = reference("result.result");
    definition.block.output = execution::loop_result_schema(&result);
    assert!(
        matches!(compile_test(definition),Err(WorkflowCompileError::InvalidField(message)) if message.contains("expanded execution"))
    );
}

#[tokio::test]
async fn nested_loops_inside_parallel_have_bounded_counts_and_inherited_identity() {
    let state = schema(json!({"passed":{"type":"boolean"}}), &["passed"]);
    let pure = serde_json::from_value(json!({"description":"nested loops","block":{
        "input":state,"output":state,"entry":"done","nodes":{"done":{"type":"return","output":{"type":"reference","path":["args"]}}},"edges":[]}})).unwrap();
    let inner = wrap_definition(pure, 2);
    let mut outer = wrap_definition(inner, 3);
    let WorkflowNodeDefinition::Loop { until, carry, .. } =
        outer.block.nodes.get_mut("feedback").unwrap()
    else {
        unreachable!()
    };
    **until = WorkflowPredicate::Boolean {
        value: WorkflowValue::Literal {
            value: json!(false),
        },
    };
    *carry = reference("result.result");
    let output = outer.block.output.clone();
    let branch = WorkflowBranch {
        input: reference("args"),
        block: outer.block,
    };
    outer.block = WorkflowBlock {
        input: state,
        output: schema(json!({"a":output,"b":output}), &["a", "b"]),
        entry: "parallel".into(),
        nodes: BTreeMap::from([
            (
                "parallel".into(),
                WorkflowNodeDefinition::Parallel {
                    branches: BTreeMap::from([("a".into(), branch.clone()), ("b".into(), branch)]),
                },
            ),
            (
                "done".into(),
                WorkflowNodeDefinition::Return {
                    output: reference("parallel"),
                },
            ),
        ]),
        edges: vec![edge("parallel", "done")],
    };
    let plane = workflow_test_plane(1);
    let runtime = workflow_runtime(&plane);
    let observations = runtime.observations.subscribe();
    let (_, cancellation) = workflow_cancellation();
    let output = runtime
        .run_foreground(
            Arc::new(compile_test(outer).unwrap()),
            ToolCallId::new("nested"),
            workflow_test_context(&plane),
            json!({"passed":false}),
            cancellation,
        )
        .await
        .unwrap();
    assert_eq!(output["a"], output["b"]);
    assert_eq!(
        output["a"],
        json!({"status":"exhausted","iterations":3,"result":{"status":"exhausted","iterations":2,"result":{"passed":false}}})
    );
    let events = observations.borrow();
    let instances = events
        .iter()
        .filter_map(|event| match event {
            RuntimeEvent::WorkflowLoopIterationAdmitted { body, .. } => Some(body.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(instances.len(), 18);
    assert_eq!(instances.iter().collect::<BTreeSet<_>>().len(), 18);
    assert!(
        instances
            .iter()
            .any(|instance| instance.invocations == [0, 0, 3, 2])
    );
}

#[tokio::test]
async fn global_step_limit_does_not_reset_at_iteration_entry() {
    let plane = workflow_test_plane(1);
    let runtime = workflow_runtime(&plane);
    let checker = Arc::new(Checker {
        starts: AtomicUsize::new(0),
        satisfied_at: 9,
        received: std::sync::Mutex::new(Vec::new()),
    });
    let context = context_with_registration(
        &plane,
        ToolRegistration::plain(definition(), checker.clone()),
        crate::agent::AttemptLifecycle::default(),
    );
    let mut program = compile_test(checker_definition(3)).unwrap();
    program.execution_bound = 5; // Loop + iteration + Tool + Return + iteration; no second Tool.
    let (_, cancellation) = workflow_cancellation();
    let error = runtime
        .run_foreground(
            Arc::new(program),
            ToolCallId::new("step-budget"),
            context,
            json!({"passed":false,"label":"initial"}),
            cancellation,
        )
        .await
        .unwrap_err();
    assert_eq!(error, WorkflowRunError::LimitExceeded(WorkflowLimit::Steps));
    assert_eq!(checker.starts.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn outer_deadline_during_second_iteration_drains_and_never_resets() {
    let plane = workflow_test_plane(1);
    let runtime = workflow_runtime(&plane);
    let observations = runtime.observations.subscribe();
    let mut probe = Probe::new(ToolExecutionStatus::Success);
    let release = Arc::new(tokio::sync::Notify::new());
    let settle = Arc::new(tokio::sync::Notify::new());
    Arc::get_mut(&mut probe).unwrap().release = Some(release.clone());
    Arc::get_mut(&mut probe).unwrap().settled = Some(settle.clone());
    let mut context = context(
        &plane,
        probe.clone(),
        crate::agent::AttemptLifecycle::default(),
    );
    let clock = Arc::new(crate::runtime::ManualMonotonicClock::new());
    let services = Arc::make_mut(context.native.as_mut().unwrap());
    services.clock = clock.clone();
    services.leaf_policy = crate::tools::deadline::ToolExecutionDeadlinePolicy::new(
        std::time::Duration::from_secs(1),
        None,
    );
    let mut started = probe.started.subscribe();
    let mut cancelled = probe.cancelled.subscribe();
    let (_, cancellation) = workflow_cancellation();
    let task = tokio::spawn(run_outer(
        runtime,
        Arc::new(compile_test(wrap_definition(program_definition(), 3)).unwrap()),
        context,
        cancellation,
    ));
    started.wait_for(|n| *n == 1).await.unwrap();
    clock.advance(60);
    release.notify_one();
    started.wait_for(|n| *n == 2).await.unwrap();
    clock.advance(40);
    cancelled.wait_for(|v| *v).await.unwrap();
    assert!(!task.is_finished());
    assert_eq!(
        observations
            .borrow()
            .iter()
            .filter(|e| matches!(e, RuntimeEvent::WorkflowLoopIterationSettled { .. }))
            .count(),
        1
    );
    settle.notify_one();
    let (result, _) = task.await.unwrap();
    assert_eq!(result.status, ToolExecutionStatus::TimedOut);
    assert_eq!(probe.starts.load(Ordering::SeqCst), 2);
    assert!(
        !observations
            .borrow()
            .iter()
            .any(|e| matches!(e, RuntimeEvent::WorkflowLoopExited { .. }))
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(clippy::too_many_lines)] // Two completion orders share one explicit native gate sequence.
async fn parallel_body_return_waits_for_siblings_and_fresh_children_in_definition_order() {
    use crate::runtime::subagent::ipc::ChildResultStatus;
    let mut outcomes = Vec::new();
    for reverse in [false, true] {
        let plane = workflow_test_plane(2);
        let mut children = (0..4)
            .map(|_| stage_workflow_child(&plane))
            .collect::<Vec<_>>();
        let state = schema(json!({"passed":{"type":"boolean"}}), &["passed"]);
        let branch = json!({"input":{"type":"reference","path":["args"]},"block":{
            "input":state,"output":state,"entry":"work","nodes":{
                "work":{"type":"agent","profile":"reviewer","task":"Check fixed input","input":{},"output":state},
                "done":{"type":"return","output":{"type":"reference","path":["work"]}}
            },"edges":[{"from":"work","to":"done"}]}});
        let definition = serde_json::from_value(json!({"description":"parallel feedback","block":{
            "input":state,"output":state,"entry":"fanout","nodes":{
                "fanout":{"type":"parallel","branches":{"a":branch,"b":branch}},
                "done":{"type":"return","output":{"type":"reference","path":["fanout","a"]}}
            },"edges":[{"from":"fanout","to":"done"}]}}))
        .unwrap();
        let runtime = workflow_runtime(&plane);
        let mut observations = runtime.observations.subscribe();
        let context = workflow_test_context(&plane);
        let (_, cancellation) = workflow_cancellation();
        let program = Arc::new(compile_test(wrap_definition(definition, 3)).unwrap());
        let task = tokio::spawn(async move {
            runtime
                .run_foreground(
                    program,
                    ToolCallId::new("parallel-loop"),
                    context,
                    json!({"passed":false}),
                    cancellation,
                )
                .await
        });
        for iteration in 1..=2 {
            let mut a = children.remove(0);
            let mut b = children.remove(0);
            a.expect_delegate().await;
            b.expect_delegate().await;
            let (first, last, key) = if reverse {
                (&mut b, &mut a, "b")
            } else {
                (&mut a, &mut b, "a")
            };
            let output = if iteration == 1 {
                r#"{"passed":false}"#
            } else {
                r#"{"passed":true}"#
            };
            first
                .send_result(ChildResultStatus::Succeeded, Some(output))
                .await;
            observations.wait_for(|events| events.iter().any(|e| matches!(e,RuntimeEvent::WorkflowBlockSettled {instance,..} if instance.definition.blocks == ["feedback","body","fanout",key] && instance.invocations == [0,iteration,0]))).await.unwrap();
            assert_eq!(
                observations
                    .borrow()
                    .iter()
                    .filter(|e| matches!(e, RuntimeEvent::WorkflowLoopIterationSettled { .. }))
                    .count(),
                iteration as usize - 1
            );
            assert!(!task.is_finished());
            last.send_result(ChildResultStatus::Succeeded, Some(output))
                .await;
        }
        outcomes.push(task.await.unwrap().unwrap());
        let events = observations.borrow();
        let agents = events
            .iter()
            .filter_map(|e| {
                if let RuntimeEvent::WorkflowAgentAdmitted {
                    node_id,
                    subagent_id,
                    ..
                } = e
                {
                    Some((node_id, subagent_id))
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        assert_eq!(agents.len(), 4);
        let unique = agents
            .iter()
            .map(|(node, _)| (*node).clone())
            .collect::<BTreeSet<_>>();
        assert_eq!(unique.len(), 4);
        for (node, child) in agents {
            assert!(
                plane
                    .registry
                    .take_workflow_agent_output(child, node)
                    .is_none(),
                "committed output transfers exactly once"
            );
        }
        assert!(plane.registry.unsettled_snapshot().is_empty());
    }
    assert_eq!(outcomes[0], outcomes[1]);
    assert_eq!(
        outcomes[0],
        json!({"status":"satisfied","iterations":2,"result":{"passed":true}})
    );
}
