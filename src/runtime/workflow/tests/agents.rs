//! An Agent's output routes and failure handling, as
//! Dify's Agent node.
use super::*;

/// A `triage` Agent whose outgoing edges and `done` return are authored;
/// every return yields `{result: string}` from the named reference.
fn agent(node: Value, exits: &[(&str, Option<WorkflowPort>, &str)]) -> WorkflowDefinition {
    let mut nodes = BTreeMap::from([("triage".into(), serde_json::from_value(node).unwrap())]);
    let mut edges = Vec::new();
    for (to, port, read) in exits {
        nodes.insert(
            (*to).into(),
            return_node(BTreeMap::from([("result".into(), reference(read))])),
        );
        edges.push(match port {
            Some(port) => branch_edge("triage", to, port.clone()),
            None => edge("triage", to),
        });
    }
    base_definition(
        "triage",
        nodes,
        edges,
        schema(json!({"result":{"type":"string"}}), &["result"]),
    )
}

fn node(extra: &Value) -> Value {
    let mut node = json!({"type":"agent","profile":"reviewer",
        "task":[{"type":"literal","value":"Triage the request."}],"output":{"type":"text"}});
    node.as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    node
}

fn routes() -> Value {
    json!([{"id":"approve","description":"The change can ship."},
        {"id":"revise","description":"The change needs edits."}])
}

#[test]
fn routes_are_exits_the_child_picks_through_a_required_switch() {
    let routed = agent(
        node(&json!({"routes":routes()})),
        &[
            (
                "shipped",
                Some(WorkflowPort::case("approve")),
                "triage.switch",
            ),
            ("edited", Some(WorkflowPort::case("revise")), "triage.text"),
        ],
    );
    let program = compile_test(routed).unwrap();
    let value = &program.node_outputs()["block.nodes.triage"];
    assert_eq!(value["required"], json!(["text", "switch"]));
    assert_eq!(
        value["properties"]["switch"]["enum"],
        json!(["approve", "revise"])
    );
    assert!(
        value["properties"]["switch"]["description"]
            .as_str()
            .unwrap()
            .contains("- revise: The change needs edits.")
    );
    let WorkflowNodeProgram::Agent(compiled) = &program.block.nodes["triage"] else {
        panic!("agent")
    };
    assert_eq!(compiled.terminal_schema().as_ref(), Some(value));

    // A declared schema keeps its fields and gains the switch.
    let structured = node(&json!({"routes":routes(),"output":{"type":"json_schema",
        "schema":schema(json!({"reason":{"type":"string"}}), &["reason"])}}));
    compile_test(agent(
        structured,
        &[
            (
                "shipped",
                Some(WorkflowPort::case("approve")),
                "triage.reason",
            ),
            (
                "edited",
                Some(WorkflowPort::case("revise")),
                "triage.switch",
            ),
        ],
    ))
    .unwrap();

    let mut reserved = routes();
    reserved[1]["id"] = json!("fail");
    let mut duplicate = routes();
    duplicate[1]["id"] = json!("approve");
    let mut undescribed = routes();
    undescribed[1]["description"] = json!(" ");
    let both = [
        (
            "shipped",
            Some(WorkflowPort::case("approve")),
            "triage.text",
        ),
        ("edited", Some(WorkflowPort::case("revise")), "triage.text"),
    ];
    for definition in [
        // Every route leads somewhere, and a routed Agent has no `next`.
        agent(node(&json!({"routes":routes()})), &both[..1]),
        agent(
            node(&json!({"routes":routes()})),
            &[
                both[0].clone(),
                both[1].clone(),
                ("plain", None, "triage.text"),
            ],
        ),
        agent(node(&json!({"routes":reserved})), &both),
        agent(node(&json!({"routes":duplicate})), &both),
        agent(node(&json!({"routes":undescribed})), &both),
        // The switch is reserved once routes exist.
        agent(
            node(&json!({"routes":routes(),"output":{"type":"json_schema",
                "schema":schema(json!({"switch":{"type":"string"}}), &[])}})),
            &both,
        ),
    ] {
        assert!(compile_test(definition).is_err());
    }
}

#[test]
fn only_the_fail_path_sees_the_error_and_defaults_satisfy_the_node_value() {
    let fail = json!({"on_error":{"type":"fail_branch"}});
    compile_test(agent(
        node(&fail),
        &[
            ("done", None, "triage.text"),
            ("recover", Some(WorkflowPort::Fail), "triage.error_message"),
        ],
    ))
    .unwrap();
    // Routes and a fail branch together: each route is a success path.
    let mut routed_fail = node(&fail);
    routed_fail["routes"] = routes();
    compile_test(agent(
        routed_fail,
        &[
            (
                "shipped",
                Some(WorkflowPort::case("approve")),
                "triage.text",
            ),
            (
                "edited",
                Some(WorkflowPort::case("revise")),
                "triage.switch",
            ),
            ("recover", Some(WorkflowPort::Fail), "triage.error_type"),
        ],
    ))
    .unwrap();
    compile_test(agent(
        node(&json!({"on_error":{"type":"default_value","value":{"text":"fallback"}}})),
        &[("done", None, "triage.text")],
    ))
    .unwrap();
    for definition in [
        agent(
            node(&fail),
            &[
                ("done", None, "triage.error_message"),
                ("recover", Some(WorkflowPort::Fail), "triage.error_message"),
            ],
        ),
        agent(
            node(&fail),
            &[
                ("done", None, "triage.text"),
                ("recover", Some(WorkflowPort::Fail), "triage.text"),
            ],
        ),
        agent(node(&fail), &[("done", None, "triage.text")]),
        agent(
            node(&json!({})),
            &[
                ("done", None, "triage.text"),
                ("recover", Some(WorkflowPort::Fail), "triage.text"),
            ],
        ),
        // A routed default names one of its routes.
        agent(
            node(
                &json!({"routes":routes(),"on_error":{"type":"default_value",
                "value":{"text":"fallback","switch":"elsewhere"}}}),
            ),
            &[
                (
                    "shipped",
                    Some(WorkflowPort::case("approve")),
                    "triage.text",
                ),
                ("edited", Some(WorkflowPort::case("revise")), "triage.text"),
            ],
        ),
        agent(
            node(&json!({"on_error":{"type":"default_value","value":{"text":1}}})),
            &[("done", None, "triage.text")],
        ),
    ] {
        assert!(compile_test(definition).is_err());
    }
}

/// Runs `definition` with one scripted child answering `result`.
#[cfg(unix)]
async fn run(
    definition: WorkflowDefinition,
    status: crate::runtime::subagent::ipc::ChildResultStatus,
    result: Option<&str>,
) -> (Result<Value, WorkflowRunError>, Vec<RuntimeEvent>) {
    let plane = workflow_test_plane(1);
    let mut child = stage_workflow_child(&plane);
    let runtime = workflow_runtime(&plane);
    let observations = runtime.observations.subscribe();
    let context = workflow_test_context(&plane);
    let (_, cancellation) = workflow_cancellation();
    let program = Arc::new(compile_test(definition).unwrap());
    let task = tokio::spawn(async move {
        runtime
            .run_test_foreground(
                program,
                ToolCallId::new("agent-routes"),
                context,
                json!({"task":"Ship it?"}),
                cancellation,
            )
            .await
    });
    child.expect_delegate().await;
    child.send_result(status, result).await;
    let outcome = task.await.unwrap();
    let events = observations.borrow().clone();
    (outcome, events)
}

#[cfg(unix)]
#[tokio::test]
async fn the_child_switch_selects_the_route_and_its_text_flows_on() {
    use crate::runtime::subagent::ipc::ChildResultStatus;
    let routed = || {
        agent(
            node(&json!({"routes":routes()})),
            &[
                (
                    "shipped",
                    Some(WorkflowPort::case("approve")),
                    "triage.switch",
                ),
                ("edited", Some(WorkflowPort::case("revise")), "triage.text"),
            ],
        )
    };
    let (outcome, events) = run(
        routed(),
        ChildResultStatus::Succeeded,
        Some(r#"{"text":"Tighten the error message.","switch":"revise"}"#),
    )
    .await;
    assert_eq!(
        outcome.unwrap(),
        json!({"result":"Tighten the error message."})
    );
    assert!(events.iter().any(|event| matches!(event,
        RuntimeEvent::WorkflowBranchSelected { port, successor, .. }
            if *port == WorkflowPort::case("revise") && successor == "edited")));
    // A switch outside the routes is the child's failure, never a guess.
    let (outcome, _) = run(
        routed(),
        ChildResultStatus::Succeeded,
        Some(r#"{"text":"Done.","switch":"elsewhere"}"#),
    )
    .await;
    assert!(outcome.is_err());
}

#[cfg(unix)]
#[tokio::test]
async fn a_failed_child_takes_the_fail_branch_or_its_default() {
    use crate::runtime::subagent::ipc::ChildResultStatus;
    let (outcome, _) = run(
        agent(
            node(&json!({"on_error":{"type":"fail_branch"}})),
            &[
                ("done", None, "triage.text"),
                ("recover", Some(WorkflowPort::Fail), "triage.error_type"),
            ],
        ),
        ChildResultStatus::Failed,
        None,
    )
    .await;
    assert_eq!(outcome.unwrap(), json!({"result":"failed"}));
    let (outcome, _) = run(
        agent(
            node(
                &json!({"routes":routes(),"on_error":{"type":"default_value",
                "value":{"text":"Needs a human look.","switch":"revise"}}}),
            ),
            &[
                (
                    "shipped",
                    Some(WorkflowPort::case("approve")),
                    "triage.switch",
                ),
                ("edited", Some(WorkflowPort::case("revise")), "triage.text"),
            ],
        ),
        ChildResultStatus::Failed,
        None,
    )
    .await;
    assert_eq!(outcome.unwrap(), json!({"result":"Needs a human look."}));
    // Without a strategy the run still stops.
    let (outcome, _) = run(
        agent(node(&json!({})), &[("done", None, "triage.text")]),
        ChildResultStatus::Failed,
        None,
    )
    .await;
    assert!(outcome.is_err());
}
