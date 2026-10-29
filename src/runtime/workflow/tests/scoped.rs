//! WF-01 regressions: every interleaving is driven by a native frame or gate.
use super::*;
use crate::durable::JournalStore;

fn literal(value: Value) -> WorkflowValue {
    WorkflowValue::Literal { value }
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
            &json!({"flag":true,"name":"input"}),
            &imbl::OrdMap::new()
        )
        .unwrap(),
        json!({"flag":true,"names":["input","fixed"]})
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
            &json!({"flag":true,"name":"input"}),
            &imbl::OrdMap::new()
        )
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
            .run_test_foreground(
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
    assert_eq!(runs[0].origin, runs[1].origin);
    assert_eq!(
        runs.iter().map(|run| run.invocation).collect::<Vec<_>>(),
        vec![1, 2]
    );
    assert_ne!(runs[0], runs[1]);
}

#[test]
fn oversized_runtime_construction_is_atomic() {
    let input = json!({"text":"x".repeat(MAX_VALUE_BYTES / 2)});
    let values = imbl::OrdMap::from(vec![("already".to_owned(), json!({"committed":true}))]);
    let expression = WorkflowValue::Object {
        fields: BTreeMap::from([
            ("first".into(), reference("args.text")),
            ("second".into(), reference("args.text")),
        ]),
    };
    assert!(evaluate_value(&expression, &input, &values).is_err());
    assert_eq!(
        values,
        imbl::OrdMap::from(vec![("already".to_owned(), json!({"committed":true}))])
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
            .run_test_foreground(
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
    observation(&mut observations, |event| matches!(event, RuntimeEvent::WorkflowNodeSettled { instance, outcome: WorkflowExecutionOutcome::Completed } if instance.node == "alpha")).await;
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
        1,
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
    assert_ne!(agents[0].0.node, agents[1].0.node);
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
            .run_test_foreground(
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

fn multi_step_program() -> Arc<WorkflowProgram> {
    let mut program = (*parallel_test_program(&["alpha", "beta"])).clone();
    program.block.input_schema = schema(json!({}), &[]);
    Arc::new(program)
}
