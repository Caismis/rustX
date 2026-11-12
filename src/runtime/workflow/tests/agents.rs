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
    base_definition("triage", nodes, edges)
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

#[test]
fn routes_reject_root_finite_business_values_without_weakening_the_contract() {
    for (keyword, value) in [
        ("const", json!({"ok":true})),
        ("enum", json!([{"ok":true},{"ok":false}])),
    ] {
        let mut schema = schema(json!({"ok":{"type":"boolean"}}), &["ok"]);
        schema[keyword] = value;
        let business = node(&json!({"output":{"type":"json_schema","schema":schema}}));
        assert!(compile_test(agent(business.clone(), &[("done", None, "triage.ok")])).is_ok());
        let mut routed = business;
        routed["routes"] = routes();
        assert!(
            authoring::declared_output(&serde_json::from_value(routed.clone()).unwrap()).is_err()
        );
        let error = compile_test(agent(
            routed,
            &[
                ("shipped", Some(WorkflowPort::case("approve")), "triage.ok"),
                ("edited", Some(WorkflowPort::case("revise")), "triage.ok"),
            ],
        ))
        .unwrap_err();
        assert!(error.to_string().contains("root const or enum"));
    }
}

#[test]
fn agent_fail_branch_requires_fail_edge_when_node_authors_permissions() {
    let fail = json!({
        "on_error":{"type":"fail_branch"},
        "permissions":{"plan":"default","automatic":"default","full_access":"default"}
    });
    assert!(compile_test(agent(
        node(&fail),
        &[("done", None, "triage.text")],
    ))
    .is_err());
    compile_test(agent(
        node(&fail),
        &[
            ("done", None, "triage.text"),
            ("recover", Some(WorkflowPort::Fail), "triage.error_message"),
        ],
    ))
    .unwrap();
}

#[cfg(unix)]
#[test]
fn agent_entry_resource_permissions_inherit_named_profile_when_node_omits() {
    use crate::runtime::permission::{ResourcePermissionSet, ResourcePermissionValue};
    let plane = workflow_test_plane(1);
    let profile_permissions = ResourcePermissionSet {
        plan: ResourcePermissionValue::Default,
        automatic: ResourcePermissionValue::Default,
        full_access: ResourcePermissionValue::Deny,
    };
    let context = workflow_test_context_for_generation(
        &plane,
        1,
        "Workflow test instructions",
        WorkflowCatalog::empty(),
        crate::runtime::types::ApprovalMode::FullAccess,
        Some(profile_permissions),
    );
    let program = compile_test(agent(
        node(&json!({})),
        &[("done", None, "triage.text")],
    ))
    .unwrap();
    let WorkflowNodeProgram::Agent(compiled) = &program.block.nodes["triage"] else {
        panic!("expected agent node");
    };
    assert_eq!(compiled.resource_permissions, None);
    let context = context.with_test_workflow(&program);
    let admitted = context
        .resources()
        .workflows()
        .executable(program.id())
        .expect("admitted workflow");
    let WorkflowNodeProgram::Agent(frozen) = &admitted.block.nodes["triage"] else {
        panic!("expected agent node");
    };
    assert_eq!(frozen.resource_permissions, Some(profile_permissions));
}

#[cfg(unix)]
#[tokio::test]
async fn agent_admit_denies_when_named_profile_entry_permission_denies_without_node_override() {
    use crate::runtime::permission::{ResourcePermissionSet, ResourcePermissionValue};
    let plane = workflow_test_plane(1);
    let mut child = stage_workflow_child(&plane);
    let runtime = workflow_runtime(&plane);
    let profile_permissions = ResourcePermissionSet {
        plan: ResourcePermissionValue::Default,
        automatic: ResourcePermissionValue::Default,
        full_access: ResourcePermissionValue::Deny,
    };
    let context = workflow_test_context_for_generation(
        &plane,
        1,
        "Workflow test instructions",
        WorkflowCatalog::empty(),
        crate::runtime::types::ApprovalMode::FullAccess,
        Some(profile_permissions),
    );
    let (_, cancellation) = workflow_cancellation();
    let program = Arc::new(compile_test(agent(
        node(&json!({})),
        &[("done", None, "triage.text")],
    ))
    .unwrap());
    let task = tokio::spawn(async move {
        runtime
            .run_test_foreground(
                program,
                ToolCallId::new("agent-profile-deny"),
                context,
                json!({"task":"Ship it?"}),
                cancellation,
            )
            .await
    });
    let outcome = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        tokio::select! {
            result = task => result.expect("workflow task"),
            () = child.expect_delegate() => panic!("child must not be admitted when profile entry is denied"),
        }
    })
    .await
    .expect("workflow should settle without admitting a child");
    let error = outcome.expect_err("denied profile entry must not complete successfully");
    assert!(
        error
            .to_string()
            .contains("denied by an effective resource permission"),
        "expected profile resource deny, got {error}"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn agent_admit_blocks_before_prepare_when_named_profile_entry_denies_plan_mode() {
    use crate::runtime::permission::{ResourcePermissionSet, ResourcePermissionValue};
    let plane = workflow_test_plane(1);
    let mut child = stage_workflow_child(&plane);
    let runtime = workflow_runtime(&plane);
    let profile_permissions = ResourcePermissionSet {
        plan: ResourcePermissionValue::Deny,
        automatic: ResourcePermissionValue::Default,
        full_access: ResourcePermissionValue::Default,
    };
    let context = workflow_test_context_for_generation(
        &plane,
        1,
        "Workflow test instructions",
        WorkflowCatalog::empty(),
        crate::runtime::types::ApprovalMode::Plan,
        Some(profile_permissions),
    );
    let (_, cancellation) = workflow_cancellation();
    let program = Arc::new(compile_test(agent(
        node(&json!({})),
        &[("done", None, "triage.text")],
    ))
    .unwrap());
    let task = tokio::spawn(async move {
        runtime
            .run_test_foreground(
                program,
                ToolCallId::new("agent-profile-plan-deny"),
                context,
                json!({"task":"Ship it?"}),
                cancellation,
            )
            .await
    });
    let outcome = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        tokio::select! {
            result = task => result.expect("workflow task"),
            () = child.expect_delegate() => panic!("child must not reach prepare when profile entry is denied"),
        }
    })
    .await
    .expect("workflow should settle without admitting a child");
    let error = outcome.expect_err("plan-mode profile deny must fail before child prepare");
    assert!(
        error
            .to_string()
            .contains("denied by an effective resource permission"),
        "expected profile plan deny, got {error}"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn agent_admit_denies_when_entry_permission_denies_current_mode() {
    use crate::runtime::permission::{ResourcePermissionSet, ResourcePermissionValue};
    let plane = workflow_test_plane(1);
    let mut child = stage_workflow_child(&plane);
    let runtime = workflow_runtime(&plane);
    let context = workflow_test_context_for_generation(
        &plane,
        1,
        "Workflow test instructions",
        WorkflowCatalog::empty(),
        crate::runtime::types::ApprovalMode::Plan,
        None,
    );
    let (_, cancellation) = workflow_cancellation();
    let mut agent_node = node(&json!({}));
    agent_node["permissions"] = json!({
        "plan": "deny",
        "automatic": "default",
        "full_access": "allow"
    });
    let mut definition = agent(agent_node, &[("done", None, "triage.text")]);
    definition
        .block
        .edges
        .retain(|edge| edge.from != "workflow_start");
    definition
        .block
        .edges
        .insert(0, edge("workflow_start", "triage"));
    let program = Arc::new(compile_test(definition).unwrap());
    let WorkflowNodeProgram::Agent(compiled) = &program.block.nodes["triage"] else {
        panic!("expected agent node");
    };
    assert_eq!(
        compiled.resource_permissions,
        Some(ResourcePermissionSet {
            plan: ResourcePermissionValue::Deny,
            automatic: ResourcePermissionValue::Default,
            full_access: ResourcePermissionValue::Allow,
        })
    );
    let task = tokio::spawn(async move {
        runtime
            .run_test_foreground(
                program,
                ToolCallId::new("agent-deny"),
                context,
                json!({"task":"Ship it?"}),
                cancellation,
            )
            .await
    });
    let outcome = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        tokio::select! {
            result = task => result.expect("workflow task"),
            () = child.expect_delegate() => panic!("child must not be admitted when entry is denied"),
        }
    })
    .await
    .expect("workflow should settle without admitting a child");
    let error = outcome.expect_err("denied agent entry must not complete successfully");
    assert!(
        error
            .to_string()
            .contains("denied by an effective resource permission"),
        "expected resource deny, got {error}"
    );
}
