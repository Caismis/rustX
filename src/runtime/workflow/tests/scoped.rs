//! WF-01 regressions: every interleaving is driven by a native frame or gate.
use super::*;

fn literal(value: Value) -> WorkflowValue {
    WorkflowValue::Literal { value }
}

fn empty_block() -> WorkflowBlock {
    WorkflowBlock {
        input: schema(json!({}), &[]),
        output: schema(json!({}), &[]),
        entry: "done".into(),
        nodes: BTreeMap::from([(
            "done".into(),
            WorkflowNodeDefinition::Return {
                output: literal(json!({})),
            },
        )]),
        edges: Vec::new(),
    }
}

#[allow(clippy::needless_pass_by_value)] // fixture builder consumes or clones authored blocks
fn wrap(block: WorkflowBlock, count: usize) -> WorkflowBlock {
    let mut parent = empty_block();
    parent.entry = "fanout".into();
    parent.nodes.insert(
        "fanout".into(),
        WorkflowNodeDefinition::Parallel {
            branches: (0..count)
                .map(|index| {
                    (
                        format!("branch_{index:02}"),
                        WorkflowBranch {
                            input: literal(json!({})),
                            block: block.clone(),
                        },
                    )
                })
                .collect(),
        },
    );
    parent.edges = vec![edge("fanout", "done")];
    parent
}

fn compile_block_fixture(block: WorkflowBlock) -> Result<WorkflowProgram, WorkflowCompileError> {
    compile_test(WorkflowDefinition {
        workspace: None,
        tools: std::collections::BTreeSet::default(),
        timeout_ms: 600_000,
        description: "scoped test".into(),
        block,
    })
}

#[test]
fn lexical_scope_rejects_parent_and_sibling_reads_with_duplicate_local_names() {
    for invalid in ["other.work.summary", "parent.summary", "args.missing"] {
        let mut branch = empty_block();
        branch.nodes.insert(
            "done".into(),
            WorkflowNodeDefinition::Return {
                output: reference(invalid),
            },
        );
        assert!(matches!(
            compile_block_fixture(wrap(branch, 2)),
            Err(ref error) if matches!(error.cause(), WorkflowCompileError::InvalidReference(_))
        ));
    }
    let program =
        compile_block_fixture(wrap(empty_block(), 2)).expect("duplicate private done ids legal");
    let WorkflowNodeProgram::Parallel { branches, .. } = &program.block.nodes["fanout"] else {
        panic!("parallel")
    };
    assert_ne!(
        branches["branch_00"].block.path,
        branches["branch_01"].block.path
    );
}

#[test]
fn typed_constructions_predicates_and_malformed_syntax_are_closed() {
    let available = SchemaMap::from_schema(&schema(
        json!({"flag":{"type":"boolean"}, "name":{"type":"string"}}),
        &["flag", "name"],
    ));
    let constructed = WorkflowValue::Object {
        fields: BTreeMap::from([
            ("flag".into(), reference("args.flag")),
            (
                "names".into(),
                WorkflowValue::Array {
                    items: vec![reference("args.name"), literal(json!("fixed"))],
                },
            ),
        ]),
    };
    let actual = value_schema(&constructed, &available, "done", 0).expect("typed construction");
    let expected = schema(
        json!({"flag":{"type":"boolean"},"names":{"type":"array","items":{"type":"string"}}}),
        &["flag", "names"],
    );
    assert!(schemas_compatible(&actual, &expected));
    assert_eq!(
        evaluate_value(
            &constructed,
            &json!({"flag":true,"name":"input"}).into(),
            &BTreeMap::new()
        )
        .unwrap(),
        json!({"flag":true,"names":["input","fixed"]}).into()
    );
    let predicate = WorkflowPredicate::And {
        predicates: vec![
            WorkflowPredicate::Boolean {
                value: reference("args.flag"),
            },
            WorkflowPredicate::Not {
                predicate: Box::new(WorkflowPredicate::NotEqual {
                    left: reference("args.name"),
                    right: literal(json!("input")),
                }),
            },
        ],
    };
    validate_predicate(&predicate, &available, "choose", 0).unwrap();
    assert!(
        evaluate_predicate(
            &predicate,
            &json!({"flag":true,"name":"input"}).into(),
            &BTreeMap::new()
        )
        .unwrap()
        .value
        .as_bool()
        .unwrap()
    );
    for invalid in [
        WorkflowPredicate::Boolean {
            value: literal(json!(1)),
        },
        WorkflowPredicate::Equal {
            left: literal(json!(1)),
            right: literal(json!("1")),
        },
        WorkflowPredicate::Equal {
            left: literal(json!(1)),
            right: literal(json!(1.0)),
        },
        WorkflowPredicate::And { predicates: vec![] },
    ] {
        assert!(validate_predicate(&invalid, &available, "choose", 0).is_err());
    }
    for invalid in [
        json!({"ref":"args.flag"}),
        json!({"type":"object","items":[]}),
        json!({"type":"literal","value":1,"ref":"args.flag"}),
    ] {
        assert!(serde_json::from_value::<WorkflowValue>(invalid).is_err());
    }
    assert!(serde_json::from_str::<WorkflowValue>(r#"{"type":"object","fields":{"x":{"type":"literal","value":1},"x":{"type":"literal","value":2}}}"#).is_err());
}

#[test]
fn nested_program_and_expression_limits_are_aggregate() {
    let mut wide = empty_block();
    wide.input = json!({"type":"object"});
    wide.output = json!({"type":"object"});
    compile_block_fixture(wide.clone()).expect("each private block fits independently");
    assert!(matches!(compile_block_fixture(wrap(wide, 32)),
        Err(ref error) if matches!(error.cause(), WorkflowCompileError::InvalidField(detail) if detail.contains("retained-data reservation"))));
    let mut leaf = empty_block();
    for index in 0..8 {
        let id = format!("step_{index}");
        let next = leaf.entry.clone();
        leaf.nodes.insert(
            id.clone(),
            WorkflowNodeDefinition::Branch {
                condition: WorkflowPredicate::Boolean {
                    value: literal(json!(true)),
                },
            },
        );
        leaf.edges.extend([
            branch_edge(&id, &next, WorkflowPort::True),
            branch_edge(&id, &next, WorkflowPort::False),
        ]);
        leaf.entry = id;
    }
    compile_block_fixture(leaf.clone()).expect("leaf individually valid");
    assert!(matches!(
        compile_block_fixture(wrap(leaf, 32)),
        Err(ref error) if matches!(error.cause(), WorkflowCompileError::InvalidField(_))
    ));
    let mut nested = empty_block();
    for _ in 0..MAX_BLOCK_DEPTH {
        nested = wrap(nested, 1);
    }
    compile_block_fixture(nested.clone()).expect("maximum depth accepted");
    assert!(compile_block_fixture(wrap(nested, 1)).is_err());
    let mut expression = literal(json!(null));
    for _ in 0..=MAX_VALUE_DEPTH {
        expression = WorkflowValue::Array {
            items: vec![expression],
        };
    }
    assert!(value_schema(&expression, &SchemaMap::default(), "deep", 0).is_err());
    let oversized = literal(json!("x".repeat(MAX_VALUE_BYTES)));
    assert!(value_schema(&oversized, &SchemaMap::default(), "large", 0).is_err());
    let bad_reference = WorkflowValue::Reference {
        path: vec!["x".repeat(65)],
    };
    assert!(value_schema(&bad_reference, &SchemaMap::default(), "reference", 0).is_err());
    let mut definition = WorkflowDefinition {
        workspace: None,
        tools: std::collections::BTreeSet::default(),
        timeout_ms: 600_000,
        description: "large aggregate".into(),
        block: wrap(empty_block(), 32),
    };
    if let WorkflowNodeDefinition::Parallel { branches } =
        definition.block.nodes.get_mut("fanout").unwrap()
    {
        for branch in branches.values_mut() {
            branch.block.nodes.insert(
                "done".into(),
                WorkflowNodeDefinition::Return {
                    output: literal(json!({"large":"x".repeat(20_000)})),
                },
            );
        }
    }
    assert!(
        matches!(compile_test(definition), Err(ref error) if matches!(error.cause(), WorkflowCompileError::InvalidField(detail) if detail.contains("aggregate program size")))
    );
}

#[test]
fn fresh_instances_reuse_the_same_static_definition_without_identity_collision() {
    let first = test_instance("same_program", "same_node");
    let mut second = first.clone();
    second.block.invocations[0] = 1;
    assert_eq!(first.block.definition, second.block.definition);
    assert_ne!(first, second);
    assert_ne!(
        workflow_event_id(&RuntimeEvent::WorkflowNodeStarted { instance: first }),
        workflow_event_id(&RuntimeEvent::WorkflowNodeStarted { instance: second })
    );
}

#[cfg(unix)]
#[tokio::test]
async fn repeated_outer_tool_correlation_does_not_reuse_run_identity() {
    let plane = workflow_test_plane(1);
    let runtime = workflow_runtime(&plane);
    for runtime in [runtime.clone(), runtime] {
        let (_, cancellation) = workflow_cancellation();
        runtime
            .run_foreground(
                return_only_program(),
                ToolCallId::new("same-model-id"),
                workflow_test_context(&plane),
                json!({"value":"kept"}),
                cancellation,
            )
            .await
            .unwrap();
    }
    let runs = plane
        .store
        .read_events(None, 64)
        .unwrap()
        .events
        .into_iter()
        .filter_map(|event| match event.event {
            RuntimeEvent::WorkflowStarted { run_id, .. } => Some(run_id),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(runs.len(), 2);
    assert_eq!(runs[0].attempt_id, runs[1].attempt_id);
    assert_eq!(
        runs.iter().map(|run| run.invocation).collect::<Vec<_>>(),
        vec![1, 2]
    );
    assert_ne!(runs[0], runs[1]);
}

#[test]
fn oversized_runtime_construction_is_atomic() {
    let input = json!({"text":"x".repeat(MAX_VALUE_BYTES / 2)});
    let values = BTreeMap::from([("already".into(), json!({"committed":true}).into())]);
    let expression = WorkflowValue::Object {
        fields: BTreeMap::from([
            ("first".into(), reference("args.text")),
            ("second".into(), reference("args.text")),
        ]),
    };
    assert!(evaluate_value(&expression, &input.into(), &values).is_err());
    assert_eq!(
        values,
        BTreeMap::from([("already".into(), json!({"committed":true}).into())])
    );
}

#[cfg(unix)]
async fn observation(
    receiver: &mut tokio::sync::watch::Receiver<Vec<RuntimeEvent>>,
    predicate: impl Fn(&RuntimeEvent) -> bool,
) -> RuntimeEvent {
    loop {
        if let Some(event) = receiver
            .borrow_and_update()
            .iter()
            .find(|event| predicate(event))
            .cloned()
        {
            return event;
        }
        receiver.changed().await.expect("runtime observation owner");
    }
}

#[cfg(unix)]
fn multi_step_program() -> Arc<WorkflowProgram> {
    let output = schema(json!({"summary":{"type":"string"}}), &["summary"]);
    let mut branch = agent_branch("Review".into(), output.clone());
    branch.block.nodes.insert(
        "choose".into(),
        WorkflowNodeDefinition::Branch {
            condition: WorkflowPredicate::Boolean {
                value: literal(json!(true)),
            },
        },
    );
    branch.block.nodes.insert(
        "alternative".into(),
        WorkflowNodeDefinition::Return {
            output: reference("work"),
        },
    );
    branch.block.edges = vec![
        edge("work", "choose"),
        branch_edge("choose", "done", WorkflowPort::True),
        branch_edge("choose", "alternative", WorkflowPort::False),
    ];
    let mut block = empty_block();
    block.output = schema(
        json!({"alpha":output.clone(),"beta":output}),
        &["alpha", "beta"],
    );
    block.entry = "fanout".into();
    block.nodes.insert(
        "fanout".into(),
        WorkflowNodeDefinition::Parallel {
            branches: BTreeMap::from([("alpha".into(), branch.clone()), ("beta".into(), branch)]),
        },
    );
    block.nodes.insert(
        "done".into(),
        WorkflowNodeDefinition::Return {
            output: reference("fanout"),
        },
    );
    block.edges = vec![edge("fanout", "done")];
    Arc::new(compile_block_fixture(block).unwrap())
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shared_executor_return_ownership_instance_routing_and_child_settlement() {
    let plane = workflow_test_plane(2);
    let mut alpha = stage_workflow_child(&plane);
    let mut beta = stage_workflow_child(&plane);
    let runtime = workflow_runtime(&plane);
    let mut observations = runtime.observations.subscribe();
    let context = workflow_test_context(&plane);
    let (_, cancellation) = workflow_cancellation();
    let task = tokio::spawn(async move {
        runtime
            .run_foreground(
                multi_step_program(),
                ToolCallId::new("outer"),
                context,
                json!({}),
                cancellation,
            )
            .await
    });
    alpha.expect_delegate().await;
    beta.expect_delegate().await;
    alpha
        .send_result(
            crate::runtime::subagent::ipc::ChildResultStatus::Succeeded,
            Some(r#"{"summary":"alpha"}"#),
        )
        .await;
    observation(&mut observations, |event| matches!(event, RuntimeEvent::WorkflowBlockSettled { instance, outcome: WorkflowExecutionOutcome::Completed } if instance.definition.blocks == ["fanout", "alpha"])).await;
    // Alpha's Return has completed its block while beta's native Agent is
    // explicitly held before output. The root cannot have terminalized.
    assert!(
        !observations
            .borrow()
            .iter()
            .any(|event| matches!(event, RuntimeEvent::WorkflowCompleted { .. }))
    );
    assert!(!task.is_finished());
    beta.send_result(
        crate::runtime::subagent::ipc::ChildResultStatus::Succeeded,
        Some(r#"{"summary":"beta"}"#),
    )
    .await;
    assert_eq!(
        task.await.unwrap().unwrap(),
        json!({"alpha":{"summary":"alpha"},"beta":{"summary":"beta"}})
    );
    let events = plane.store.read_events(None, 256).unwrap().events;
    let block_starts = events
        .iter()
        .filter_map(|event| match &event.event {
            RuntimeEvent::WorkflowBlockStarted { instance } => Some(instance),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        block_starts.len(),
        3,
        "root and branches enter the one execute_block seam"
    );
    assert!(
        block_starts
            .iter()
            .any(|instance| instance.definition.blocks.is_empty())
    );
    let agents = events
        .iter()
        .filter_map(|event| match &event.event {
            RuntimeEvent::WorkflowAgentAdmitted {
                node_id,
                subagent_id,
                ..
            } => Some((node_id, subagent_id)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(agents.len(), 2);
    assert_eq!(agents[0].0.node, agents[1].0.node);
    assert_ne!(agents[0].0, agents[1].0);
    for (instance, child) in agents {
        let terminal = events.iter().position(|event| matches!(&event.event, RuntimeEvent::SubagentTerminalSettled { subagent_id, .. } if subagent_id == child)).unwrap();
        let block_terminal = events.iter().position(|event| matches!(&event.event, RuntimeEvent::WorkflowBlockSettled { instance: block, .. } if block == &instance.block)).unwrap();
        let run_terminal = events
            .iter()
            .position(|event| matches!(event.event, RuntimeEvent::WorkflowCompleted { .. }))
            .unwrap();
        assert!(terminal < block_terminal && block_terminal < run_terminal);
        assert_eq!(events.iter().filter(|event| matches!(&event.event, RuntimeEvent::WorkflowAgentOutputCommitted { node_id, subagent_id, .. } if node_id == instance && subagent_id == child)).count(),1);
        assert_eq!(events.iter().filter(|event| matches!(&event.event, RuntimeEvent::WorkflowBlockSettled { instance: block, .. } if block == &instance.block)).count(),1);
    }
    assert!(matches!(
        events.last().unwrap().event,
        RuntimeEvent::WorkflowCompleted { .. }
    ));
    assert!(plane.registry.unsettled_snapshot().is_empty());
}

#[cfg(unix)]
#[tokio::test]
async fn cancellation_committed_before_node_admission_frontier_starts_no_child() {
    let plane = workflow_test_plane(1);
    let runtime = workflow_runtime(&plane);
    let context = workflow_test_context(&plane);
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    *runtime.node_frontier.lock().unwrap() = Some(execution::NodeFrontierHook {
        node: "work".into(),
        entered: entered_tx,
        release: release_rx,
    });
    let branch = agent_branch(
        "must never start".into(),
        schema(json!({"summary":{"type":"string"}}), &["summary"]),
    );
    let program = Arc::new(compile_block_fixture(branch.block).unwrap());
    let (signal, cancellation) = workflow_cancellation();
    let task = tokio::spawn(async move {
        runtime
            .run_foreground(
                program,
                ToolCallId::new("outer"),
                context,
                json!({}),
                cancellation,
            )
            .await
    });
    entered_rx.await.unwrap();
    signal.cancel();
    release_tx.send(()).unwrap();
    assert!(task.await.unwrap().unwrap_err().is_cancelled());
    assert!(
        plane.registry.all_snapshots().is_empty(),
        "zero child admissions, hence zero native model starts"
    );
    assert!(
        !plane
            .store
            .read_events(None, 64)
            .unwrap()
            .events
            .iter()
            .any(|event| matches!(event.event, RuntimeEvent::WorkflowNodeStarted { .. }))
    );
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancellation_wakes_native_capacity_waiter_and_drains_owned_child() {
    let plane = workflow_test_plane(1);
    let mut alpha = stage_workflow_child(&plane);
    let beta = stage_workflow_child(&plane);
    let waiting = plane.registry.watch_next_capacity_wait();
    let runtime = workflow_runtime(&plane);
    let context = workflow_test_context(&plane);
    let (signal, cancellation) = workflow_cancellation();
    let task = tokio::spawn(async move {
        runtime
            .run_foreground(
                multi_step_program(),
                ToolCallId::new("outer"),
                context,
                json!({}),
                cancellation,
            )
            .await
    });
    alpha.expect_delegate().await;
    waiting
        .await
        .expect("beta reached actual native capacity wait");
    signal.cancel();
    alpha.cancel_after_delegate().await;
    assert!(task.await.unwrap().unwrap_err().is_cancelled());
    assert_eq!(plane.registry.all_snapshots().len(), 1);
    assert!(plane.registry.unsettled_snapshot().is_empty());
    assert!(
        !alpha.root.exists() && !beta.root.exists(),
        "owned child settled and pending staged child rolled back"
    );
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn nested_capacity_one_blocks_never_reserve_descendant_capacity() {
    let plane = workflow_test_plane(1);
    let mut alpha = stage_workflow_child(&plane);
    let mut beta = stage_workflow_child(&plane);
    let runtime = workflow_runtime(&plane);
    let context = workflow_test_context(&plane);
    let (_, cancellation) = workflow_cancellation();
    let branch = agent_branch(
        "nested".into(),
        schema(json!({"summary":{"type":"string"}}), &["summary"]),
    );
    let nested = wrap(branch.block, 2);
    let program = Arc::new(compile_block_fixture(wrap(nested, 1)).unwrap());
    let task = tokio::spawn(async move {
        runtime
            .run_foreground(
                program,
                ToolCallId::new("outer"),
                context,
                json!({}),
                cancellation,
            )
            .await
    });
    alpha.expect_delegate().await;
    alpha
        .send_result(
            crate::runtime::subagent::ipc::ChildResultStatus::Succeeded,
            Some(r#"{"summary":"alpha"}"#),
        )
        .await;
    beta.expect_delegate().await;
    beta.send_result(
        crate::runtime::subagent::ipc::ChildResultStatus::Succeeded,
        Some(r#"{"summary":"beta"}"#),
    )
    .await;
    assert_eq!(task.await.unwrap().unwrap(), json!({}));
    assert!(plane.registry.unsettled_snapshot().is_empty());
    assert_eq!(plane.registry.all_snapshots().len(), 2);
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reversing_completions_preserves_keyed_outputs_failures_and_join_commit_order() {
    for failed in [false, true] {
        let mut observations_by_order = Vec::new();
        for reverse in [false, true] {
            let plane = workflow_test_plane(2);
            let mut alpha = stage_workflow_child(&plane);
            let mut beta = stage_workflow_child(&plane);
            let runtime = workflow_runtime(&plane);
            let context = workflow_test_context(&plane);
            let (_, cancellation) = workflow_cancellation();
            let task = tokio::spawn(async move {
                runtime
                    .run_foreground(
                        multi_step_program(),
                        ToolCallId::new("outer"),
                        context,
                        json!({}),
                        cancellation,
                    )
                    .await
            });
            alpha.expect_delegate().await;
            beta.expect_delegate().await;
            let status = if failed {
                crate::runtime::subagent::ipc::ChildResultStatus::Failed
            } else {
                crate::runtime::subagent::ipc::ChildResultStatus::Succeeded
            };
            let (first, second, ordinal, first_value, second_value) = if reverse {
                (
                    &mut beta,
                    &mut alpha,
                    2,
                    r#"{"summary":"beta"}"#,
                    r#"{"summary":"alpha"}"#,
                )
            } else {
                (
                    &mut alpha,
                    &mut beta,
                    1,
                    r#"{"summary":"alpha"}"#,
                    r#"{"summary":"beta"}"#,
                )
            };
            first
                .send_result(status, (!failed).then_some(first_value))
                .await;
            plane
                .registry
                .wait_until_settled(&SubagentId::for_conversation(
                    &plane.conversation_id,
                    ordinal,
                ))
                .await
                .unwrap();
            // The second result cannot reach native settlement until this
            // explicit send, proving both opposite terminal interleavings.
            second
                .send_result(status, (!failed).then_some(second_value))
                .await;
            let result = task.await.unwrap();
            let commits = plane
                .store
                .read_events(None, 256)
                .unwrap()
                .events
                .into_iter()
                .filter_map(|event| match event.event {
                    RuntimeEvent::WorkflowParallelSettled {
                        succeeded, failed, ..
                    } => Some((succeeded, failed)),
                    _ => None,
                })
                .collect::<Vec<_>>();
            observations_by_order.push((result, commits));
        }
        assert_eq!(observations_by_order[0], observations_by_order[1]);
    }
}
