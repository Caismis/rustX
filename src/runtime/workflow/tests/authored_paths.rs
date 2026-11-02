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
    )
}

fn branch() -> WorkflowNodeDefinition {
    WorkflowNodeDefinition::Branch {
        cases: vec![crate::runtime::workflow::WorkflowBranchCase {
            id: "true".into(),
            condition: WorkflowPredicate::Boolean {
                value: WorkflowValue::Literal { value: json!(true) },
            },
        }],
    }
}

fn loop_node() -> WorkflowNodeDefinition {
    serde_json::from_value(json!({"type":"loop","loop_count":1,"body":{
        "nodes":{"workflow_start":{"type":"start"},"stop":{"type":"loop_end"}},
        "edges":[{"from":"workflow_start","to":"stop"}]}}))
    .unwrap()
}

#[test]
fn dangling_from_has_exact_authored_edge_path() {
    let error = compile_test(graph(branch(), vec![edge("missing", "done")])).unwrap_err();
    assert_eq!(error.path(), "block.edges.1.from");
    assert!(matches!(
        error.cause(),
        WorkflowCompileError::DanglingReference(_)
    ));
}

#[test]
fn dangling_to_has_exact_authored_edge_path() {
    let error = compile_test(graph(branch(), vec![edge("route", "missing")])).unwrap_err();
    assert_eq!(error.path(), "block.edges.1.to");
    assert!(matches!(
        error.cause(),
        WorkflowCompileError::DanglingReference(_)
    ));
}

#[test]
fn invalid_branch_loop_and_ordinary_ports_have_exact_edge_paths() {
    for (node, ports) in [
        (branch(), vec![None, Some(WorkflowPort::Next)]),
        (loop_node(), vec![Some(WorkflowPort::True)]),
        (
            agent(schema(json!({}), &[])),
            vec![Some(WorkflowPort::False), Some(WorkflowPort::Next)],
        ),
    ] {
        for port in ports {
            let mut authored_edge = edge("route", "done");
            authored_edge.port = port;
            let error = compile_test(graph(node.clone(), vec![authored_edge])).unwrap_err();
            assert_eq!(error.path(), "block.edges.1.port");
            assert!(matches!(
                error.cause(),
                WorkflowCompileError::InvalidBranch(_) | WorkflowCompileError::InvalidField(_)
            ));
        }
    }
}

#[test]
fn duplicate_connection_identifies_later_authored_edge_before_sorting() {
    for (node, port) in [(branch(), WorkflowPort::True)] {
        for targets in [["done", "done"], ["other", "other"]] {
            let mut definition = graph(
                node.clone(),
                targets
                    .iter()
                    .map(|to| branch_edge("route", to, port.clone()))
                    .collect(),
            );
            definition
                .block
                .nodes
                .insert("other".into(), return_node(BTreeMap::new()));
            let error = compile_test(definition).unwrap_err();
            assert_eq!(error.path(), "block.edges.2.port");
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
        let error =
            compile_test(base_definition("entry", nodes.into_iter().collect(), edges)).unwrap_err();
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
    body.edges.push(edge("stop", "missing"));
    let error = compile_test(graph(node, vec![edge("route", "done")])).unwrap_err();
    assert_eq!(error.path(), "block.nodes.route.body.edges.1.to");
    assert!(matches!(
        error.cause(),
        WorkflowCompileError::DanglingReference(_)
    ));
}
