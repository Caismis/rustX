//! Compiler-owned CFG-06 paths, including authored edge order and lexical scopes.
use super::*;

fn graph(node: WorkflowNodeDefinition, edges: Vec<WorkflowEdgeDefinition>) -> WorkflowDefinition {
    base_definition(
        "route",
        BTreeMap::from([
            ("route".into(), node),
            ("done".into(), return_node(BTreeMap::new())),
        ]),
        edges,
        schema(json!({}), &[]),
    )
}

fn branch() -> WorkflowNodeDefinition {
    WorkflowNodeDefinition::Branch {
        condition: WorkflowPredicate::Boolean {
            value: WorkflowValue::Literal { value: json!(true) },
        },
    }
}

fn loop_node() -> WorkflowNodeDefinition {
    WorkflowNodeDefinition::Loop {
        input: WorkflowValue::Literal { value: json!({}) },
        body: Box::new(WorkflowBlock {
            input: schema(json!({}), &[]),
            output: schema(json!({}), &[]),
            entry: "done".into(),
            nodes: BTreeMap::from([("done".into(), return_node(BTreeMap::new()))]),
            edges: vec![],
        }),
        until: Box::new(WorkflowPredicate::Boolean {
            value: WorkflowValue::Literal { value: json!(true) },
        }),
        carry: WorkflowValue::Literal { value: json!({}) },
        max_iterations: 1,
    }
}

#[test]
fn dangling_from_has_exact_authored_edge_path() {
    let error = compile_test(graph(branch(), vec![edge("missing", "done")])).unwrap_err();
    assert_eq!(error.path(), "block.edges.0.from");
    assert!(matches!(
        error.cause(),
        WorkflowCompileError::DanglingReference(_)
    ));
}

#[test]
fn dangling_to_has_exact_authored_edge_path() {
    let error = compile_test(graph(branch(), vec![edge("route", "missing")])).unwrap_err();
    assert_eq!(error.path(), "block.edges.0.to");
    assert!(matches!(
        error.cause(),
        WorkflowCompileError::DanglingReference(_)
    ));
}

#[test]
fn invalid_branch_loop_and_ordinary_ports_have_exact_edge_paths() {
    for (node, ports) in [
        (branch(), vec![None, Some(WorkflowPort::Satisfied)]),
        (loop_node(), vec![None, Some(WorkflowPort::True)]),
        (
            agent(schema(json!({}), &[])),
            vec![Some(WorkflowPort::False), Some(WorkflowPort::Next)],
        ),
    ] {
        for port in ports {
            let mut authored_edge = edge("route", "done");
            authored_edge.port = port;
            let error = compile_test(graph(node.clone(), vec![authored_edge])).unwrap_err();
            assert_eq!(error.path(), "block.edges.0.port");
            assert!(matches!(
                error.cause(),
                WorkflowCompileError::InvalidBranch(_) | WorkflowCompileError::InvalidField(_)
            ));
        }
    }
}

#[test]
fn duplicate_port_identifies_later_authored_edge_before_sorting() {
    for (node, port) in [
        (branch(), WorkflowPort::True),
        (loop_node(), WorkflowPort::Satisfied),
    ] {
        for targets in [["done", "other"], ["other", "done"]] {
            let mut definition = graph(
                node.clone(),
                targets
                    .iter()
                    .map(|to| branch_edge("route", to, port))
                    .collect(),
            );
            definition
                .block
                .nodes
                .insert("other".into(), return_node(BTreeMap::new()));
            let error = compile_test(definition).unwrap_err();
            assert_eq!(error.path(), "block.edges.1.port");
            let WorkflowCompileError::InvalidBranch(reason) = error.cause() else {
                panic!("{error:?}")
            };
            assert!(reason.contains("route"));
            assert!(reason.contains(&format!("{port:?}")));
        }
    }
}

#[test]
fn cycle_selects_lexicographically_smallest_kahn_residual() {
    for reverse in [false, true] {
        let mut nodes = vec![
            ("entry".into(), agent(schema(json!({}), &[]))),
            ("a".into(), agent(schema(json!({}), &[]))),
            ("b".into(), agent(schema(json!({}), &[]))),
        ];
        let mut edges = vec![edge("entry", "b"), edge("b", "a"), edge("a", "b")];
        if reverse {
            nodes.reverse();
            edges.reverse();
        }
        let error = compile_test(base_definition(
            "entry",
            nodes.into_iter().collect(),
            edges,
            schema(json!({}), &[]),
        ))
        .unwrap_err();
        assert_eq!(error.path(), "block.nodes.a");
        assert_eq!(
            error.cause(),
            &WorkflowCompileError::Cycle { node: "a".into() }
        );
    }
}

#[test]
fn empty_and_oversized_descriptions_have_field_paths() {
    for description in [" \n".into(), "x".repeat(4097)] {
        let mut definition = graph(branch(), vec![]);
        definition.description = description;
        let error = compile_test(definition).unwrap_err();
        assert_eq!(error.path(), "description");
        assert!(matches!(
            error.cause(),
            WorkflowCompileError::InvalidField(_)
        ));
    }
}

#[test]
fn loop_body_edge_path_composes_with_parent_context() {
    let mut node = loop_node();
    let WorkflowNodeDefinition::Loop { body, .. } = &mut node else {
        unreachable!()
    };
    body.edges.push(edge("done", "missing"));
    let error = compile_test(graph(
        node,
        vec![
            branch_edge("route", "done", WorkflowPort::Satisfied),
            branch_edge("route", "done", WorkflowPort::Exhausted),
        ],
    ))
    .unwrap_err();
    assert_eq!(error.path(), "block.nodes.route.body.edges.0.to");
    assert!(matches!(
        error.cause(),
        WorkflowCompileError::DanglingReference(_)
    ));
}

#[test]
fn structural_node_errors_keep_known_authored_locations() {
    let mut invalid_id = graph(branch(), vec![]);
    invalid_id.block.nodes.insert("bad.id".into(), branch());
    assert_eq!(
        compile_test(invalid_id).unwrap_err().path(),
        "block.nodes.bad.id"
    );
    for node in [branch(), loop_node(), agent(schema(json!({}), &[]))] {
        let mut definition = graph(node, vec![]);
        definition.block.nodes.remove("done");
        assert_eq!(
            compile_test(definition).unwrap_err().path(),
            "block.nodes.route"
        );
    }
    let definition = graph(return_node(BTreeMap::new()), vec![edge("route", "done")]);
    assert_eq!(
        compile_test(definition).unwrap_err().path(),
        "block.nodes.route"
    );
    let node = WorkflowNodeDefinition::Parallel {
        branches: BTreeMap::from([(
            "bad.key".into(),
            WorkflowBranch {
                input: WorkflowValue::Literal { value: json!({}) },
                block: graph(branch(), vec![]).block,
            },
        )]),
    };
    assert_eq!(
        compile_test(graph(node, vec![edge("route", "done")]))
            .unwrap_err()
            .path(),
        "block.nodes.route.branches.bad.key"
    );
}
