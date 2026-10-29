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
        cases: vec![crate::runtime::workflow::WorkflowBranchCase {
            id: "true".into(),
            condition: WorkflowPredicate::Boolean {
                value: WorkflowValue::Literal { value: json!(true) },
            },
        }],
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
        messages: prompt("Return text", vec![text("Go.")]),
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

#[cfg(unix)]
#[tokio::test]
async fn ordered_cases_take_only_the_first_match_and_otherwise_else() {
    for (first, second, expected) in [
        (true, true, "if"),
        (false, true, "elif"),
        (false, false, "else"),
    ] {
        let mut definition = sibling_graph();
        definition.block.nodes = BTreeMap::from([
            ("start".into(), WorkflowNodeDefinition::Start {}),
            (
                "choose".into(),
                WorkflowNodeDefinition::Branch {
                    cases: vec![
                        WorkflowBranchCase {
                            id: "true".into(),
                            condition: WorkflowPredicate::Boolean {
                                value: WorkflowValue::Literal {
                                    value: json!(first),
                                },
                            },
                        },
                        WorkflowBranchCase {
                            id: "second".into(),
                            condition: WorkflowPredicate::Boolean {
                                value: WorkflowValue::Literal {
                                    value: json!(second),
                                },
                            },
                        },
                    ],
                },
            ),
        ]);
        definition.block.edges = vec![edge("start", "choose")];
        for (id, port) in [
            ("if", WorkflowPort::True),
            ("elif", WorkflowPort::case("second")),
            ("else", WorkflowPort::False),
        ] {
            definition.block.nodes.insert(
                id.into(),
                WorkflowNodeDefinition::Return {
                    output: WorkflowValue::Literal {
                        value: json!({"message":id}),
                    },
                },
            );
            definition.block.edges.push(branch_edge("choose", id, port));
        }
        let plane = workflow_test_plane(1);
        let runtime = workflow_runtime(&plane);
        let (_, cancellation) = workflow_cancellation();
        let result = runtime
            .run_test_foreground(
                Arc::new(compile_test(definition.clone()).unwrap()),
                ToolCallId::new("ordered-cases"),
                workflow_test_context(&plane),
                json!({}),
                cancellation,
            )
            .await
            .unwrap();
        assert_eq!(result, json!({"message":expected}));
        let nodes = runtime.read_model.snapshot().runs[0]
            .instances
            .iter()
            .filter_map(|row| row.node.clone())
            .collect::<Vec<_>>();
        assert_eq!(nodes, ["start", "choose", expected]);
        let WorkflowNodeDefinition::Branch { cases } =
            definition.block.nodes.get_mut("choose").unwrap()
        else {
            unreachable!()
        };
        cases[1].id = "true".into();
        assert!(
            compile_test(definition).is_err(),
            "case identities must remain unique"
        );
    }
}
