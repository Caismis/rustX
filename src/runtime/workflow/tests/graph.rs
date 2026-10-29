//! Multi-edge graphs use native nodes; no authored container owns parallelism.
use super::*;
fn literal_return() -> WorkflowNodeDefinition {
    WorkflowNodeDefinition::Return {
        output: WorkflowValue::Literal {
            value: json!({"message":"ok"}),
        },
    }
}
fn decision() -> WorkflowNodeDefinition {
    WorkflowNodeDefinition::Branch {
        condition: WorkflowPredicate::Boolean {
            value: WorkflowValue::Literal { value: json!(true) },
        },
    }
}
fn sibling_graph() -> WorkflowDefinition {
    WorkflowDefinition {
        description: "Concurrent paths and one join".into(),
        timeout_ms: 60_000,
        block: WorkflowBlock {
            input: schema(json!({}), &[]),
            output: schema(json!({"message":{"type":"string"}}), &["message"]),
            nodes: BTreeMap::from([
                ("start".into(), WorkflowNodeDefinition::Start {}),
                ("left".into(), decision()),
                ("right".into(), decision()),
                ("done".into(), literal_return()),
                ("skipped".into(), literal_return()),
            ]),
            edges: vec![
                edge("start", "left"),
                edge("start", "right"),
                branch_edge("left", "done", WorkflowPort::True),
                branch_edge("left", "skipped", WorkflowPort::False),
                branch_edge("right", "done", WorkflowPort::True),
                branch_edge("right", "skipped", WorkflowPort::False),
            ],
        },
    }
}
#[test]
fn real_start_and_every_port_allow_distinct_targets_but_never_duplicate_edges() {
    let mut definition = sibling_graph();
    definition
        .block
        .nodes
        .insert("also".into(), literal_return());
    definition
        .block
        .edges
        .push(branch_edge("left", "also", WorkflowPort::True));
    assert!(compile_test(definition.clone()).is_ok());
    definition
        .block
        .edges
        .push(branch_edge("left", "also", WorkflowPort::True));
    assert!(compile_test(definition).is_err());
    let mut definition = sibling_graph();
    definition.block.nodes.remove("start");
    assert!(matches!(
        compile_test(definition).unwrap_err().cause(),
        WorkflowCompileError::InvalidEntry(_)
    ));
}
#[test]
fn join_sees_both_parallel_producers_but_not_an_optional_conditional_producer() {
    let model = WorkflowNodeDefinition::Model {
        model: WorkflowModelPolicy::Session,
        instruction: "Return text".into(),
        input: BTreeMap::new(),
        output: WorkflowModelOutput::Text {},
    };
    let mut definition = sibling_graph();
    definition.block.nodes = BTreeMap::from([
        ("start".into(), WorkflowNodeDefinition::Start {}),
        ("left".into(), model.clone()),
        ("right".into(), model),
        (
            "done".into(),
            return_node(BTreeMap::from([
                ("left".into(), reference("left")),
                ("right".into(), reference("right")),
            ])),
        ),
    ]);
    definition.block.output = schema(
        json!({"left":{"type":"string"},"right":{"type":"string"}}),
        &["left", "right"],
    );
    definition.block.edges = vec![
        edge("start", "left"),
        edge("start", "right"),
        edge("left", "done"),
        edge("right", "done"),
    ];
    assert!(compile_test(definition.clone()).is_ok());
    definition.block.nodes.insert("choose".into(), decision());
    definition.block.edges = vec![
        edge("start", "choose"),
        branch_edge("choose", "left", WorkflowPort::True),
        branch_edge("choose", "right", WorkflowPort::False),
        edge("left", "done"),
        edge("right", "done"),
    ];
    assert!(matches!(
        compile_test(definition).unwrap_err().cause(),
        WorkflowCompileError::InvalidReference(_)
    ));
}
#[test]
fn removed_parallel_type_and_entry_field_are_rejected_at_the_file_boundary() {
    let graph = sibling_graph();
    let mut value = serde_json::to_value(&graph).unwrap();
    value["block"]["entry"] = json!("left");
    assert!(serde_json::from_value::<WorkflowDefinition>(value).is_err());
    let mut value = serde_json::to_value(graph).unwrap();
    value["block"]["nodes"]["left"] = json!({"type":"parallel","branches":{}});
    assert!(serde_json::from_value::<WorkflowDefinition>(value).is_err());
}
#[cfg(unix)]
#[tokio::test]
async fn joins_wait_for_selected_paths_run_once_and_skip_inactive_returns() {
    let plane = workflow_test_plane(1);
    let runtime = workflow_runtime(&plane);
    let context = workflow_test_context(&plane);
    let (_, cancellation) = workflow_cancellation();
    let result = runtime
        .run_test_foreground(
            Arc::new(compile_test(sibling_graph()).unwrap()),
            ToolCallId::new("graph-join"),
            context,
            json!({}),
            cancellation,
        )
        .await
        .unwrap();
    assert_eq!(result, json!({"message":"ok"}));
    let snapshot = runtime.read_model.snapshot();
    let rows = &snapshot.runs[0].instances;
    assert_eq!(
        rows.iter()
            .filter(|row| row.node.as_deref() == Some("done"))
            .count(),
        1
    );
    assert!(
        !rows
            .iter()
            .any(|row| row.node.as_deref() == Some("skipped"))
    );
    assert!(
        rows.iter()
            .any(|row| row.kind == read_model::WorkflowNodeKind::Start)
    );
}
