//! Shipped authoring entry points use the native loader, compiler and executor.
use super::*;

pub(super) fn template(id: &str) -> Arc<WorkflowProgram> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples/local-runtime/workflow-templates");
    let profiles = serde_json::from_value(json!({ "workflow":["reviewer"]})).unwrap();
    // Canonical role parsing is part of this fixture, not a replacement inline role format.
    let (roles, sources) = crate::local_runtime::agent_resources::load(
        &root,
        &root.join("absent-user-roles"),
        &profiles,
    )
    .unwrap();
    assert_eq!(roles.len(), 1);
    assert_eq!(sources[&profile("reviewer")].layer, "project");
    let catalog = crate::local_runtime::workflow_resources::load(
        &root,
        &profiles,
        &crate::local_runtime::agent_resources::load(&root, &root.join("user-agents"), &profiles)
            .unwrap()
            .0,
    )
    .unwrap();
    catalog
        .get(&WorkflowId::parse(id).unwrap())
        .unwrap()
        .clone()
}

#[test]
fn cfg237_templates_use_native_loader_compiler_and_editor_schema() {
    let editor = crate::local_runtime::schemas::generate()["workflow.schema.json"].clone();
    let validator = jsonschema::Validator::new(&editor).unwrap();
    for id in ["typed_agent", "parallel_checks", "human_plan"] {
        let program = template(id);
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!(
            "examples/local-runtime/workflow-templates/.agents/workflows/{id}.yaml"
        ));
        let definition: WorkflowDefinition =
            serde_yaml::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let mut shape = serde_json::to_value(definition).unwrap();
        assert!(validator.is_valid(&shape));
        shape["block"]["unknown"] = json!(true);
        assert!(!validator.is_valid(&shape));
        assert!(serde_json::from_value::<WorkflowDefinition>(shape).is_err());
        let view = program.inspect();
        assert_eq!(view.identity, program.tool_identity());
        assert_eq!(view.blocks["block"].entry, program.entry());
        assert_eq!(view.conservative_steps, program.execution_bound);
        assert_eq!(view.conservative_retained_bytes, program.retained_bound);
        assert!(view.workspace.is_none());
        assert!(
            !view
                .runtime_requirements
                .contains("workspace_candidate_acquisition_and_identity")
        );
        assert!(!view.runtime_requirements.contains("actual_loop_iterations"));
        assert_eq!(
            view.runtime_requirements
                .contains("human_review_decision_and_interaction_availability"),
            id == "human_plan"
        );
    }
}

#[test]
fn cfg237_loop_and_control_failures_keep_authored_paths() {
    let text = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("examples/local-runtime/workflow-templates/.agents/workflows/human_plan.yaml"),
    )
    .unwrap();
    let body: WorkflowDefinition = serde_yaml::from_str(&text).unwrap();
    let original = json!({"description":"Bounded review loop", "block":{
        "input":body.block.input, "output":body.block.output, "entry":"revise",
        "nodes":{
            "revise":{"type":"loop", "input":{"type":"reference","path":["args"]}, "body":body.block,
                "until":{"type":"boolean","value":{"type":"reference","path":["result","accepted"]}},
                "carry":{"type":"literal","value":{"plan":{"summary":"Retry"}}}, "max_iterations":2},
            "done":{"type":"return","output":{"type":"reference","path":["revise","result"]}}
        }, "edges":[{"from":"revise","to":"done","port":"satisfied"},{"from":"revise","to":"done","port":"exhausted"}]
    }});
    let compile = |value| {
        WorkflowProgram::compile(
            WorkflowId::parse("review_loop").unwrap(),
            serde_json::from_value(value).unwrap(),
            &BTreeSet::new(),
        )
    };
    let valid = compile(original.clone()).unwrap();
    assert_eq!(
        valid.inspect().blocks["block"].nodes["revise"].max_iterations,
        Some(2)
    );
    assert!(
        valid
            .inspect()
            .blocks
            .contains_key("block.nodes.revise.body")
    );
    for (pointer, value, expected) in [
        (
            "/block/nodes/revise/max_iterations",
            json!(257),
            "block.nodes.revise.max_iterations",
        ),
        (
            "/block/nodes/revise/body/nodes/review_plan/subject/value/path",
            json!(["revise", "result"]),
            "block.nodes.revise.body.nodes.review_plan.subject",
        ),
        (
            "/block/nodes/revise/carry",
            json!({"type":"literal","value":{"wrong":true}}),
            "block.nodes.revise.carry",
        ),
        (
            "/block/nodes/revise/body/edges",
            json!([]),
            "block.nodes.revise.body.nodes.return_decision",
        ),
    ] {
        let mut broken = original.clone();
        *broken.pointer_mut(pointer).unwrap() = value;
        assert_eq!(compile(broken).unwrap_err().path(), expected);
    }
}

#[test]
fn cfg237_compiled_explanation_ignores_yaml_map_insertion_order() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples/local-runtime/workflow-templates/.agents/workflows/parallel_checks.yaml");
    let text = std::fs::read_to_string(path).unwrap();
    let mut reordered: serde_yaml::Value = serde_yaml::from_str(&text).unwrap();
    let nodes = reordered["block"]["nodes"].as_mapping_mut().unwrap();
    *nodes = nodes
        .clone()
        .into_iter()
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    let branches = reordered["block"]["nodes"]["check_text"]["branches"]
        .as_mapping_mut()
        .unwrap();
    *branches = branches
        .clone()
        .into_iter()
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    let reordered = serde_yaml::to_string(&reordered).unwrap();
    assert_ne!(text, reordered);
    let compile = |text: &str| {
        WorkflowProgram::compile(
            WorkflowId::parse("parallel_checks").unwrap(),
            serde_yaml::from_str(text).unwrap(),
            &BTreeSet::from([profile("reviewer")]),
        )
        .unwrap()
    };
    let first = compile(&text);
    let second = compile(&reordered);
    assert_eq!(
        serde_json::to_value(first.inspect()).unwrap(),
        serde_json::to_value(second.inspect()).unwrap()
    );
    assert_eq!(first.outgoing("check_text"), second.outgoing("check_text"));
}

#[cfg(unix)]
#[tokio::test]
async fn cfg237_typed_agent_template_executes_native_typed_terminal() {
    let plane = workflow_test_plane(1);
    let mut child = stage_workflow_child(&plane);
    let runtime = workflow_runtime(&plane);
    let observations = runtime.observations.subscribe();
    let context = workflow_test_context(&plane);
    let (_, cancellation) = workflow_cancellation();
    let task = tokio::spawn(async move {
        runtime
            .run_foreground(
                template("typed_agent"),
                ToolCallId::new("template-agent"),
                context,
                json!({"topic":"typed values"}),
                cancellation,
            )
            .await
    });
    child.expect_delegate().await;
    child
        .send_result(
            crate::runtime::subagent::ipc::ChildResultStatus::Succeeded,
            Some(r#"{"summary":"Values have explicit contracts."}"#),
        )
        .await;
    assert_eq!(
        task.await.unwrap().unwrap(),
        json!({"summary":"Values have explicit contracts."})
    );
    assert_eq!(
        observations
            .borrow()
            .iter()
            .filter(|event| matches!(event, RuntimeEvent::WorkflowCompleted { .. }))
            .count(),
        1
    );
    assert!(matches!(
        observations.borrow().last(),
        Some(RuntimeEvent::WorkflowCompleted { .. })
    ));
}

#[cfg(unix)]
#[tokio::test]
async fn cfg237_parallel_template_preserves_keys_under_reverse_completion() {
    for reverse in [false, true] {
        let plane = workflow_test_plane(2);
        let mut brevity = stage_workflow_child(&plane);
        let mut clarity = stage_workflow_child(&plane);
        let program = template("parallel_checks");
        let before = serde_json::to_value(program.inspect()).unwrap();
        let frozen = program.clone();
        let runtime = workflow_runtime(&plane);
        let mut observations = runtime.observations.subscribe();
        let context = workflow_test_context(&plane);
        let (_, cancellation) = workflow_cancellation();
        let task = tokio::spawn(async move {
            runtime
                .run_foreground(
                    program,
                    ToolCallId::new("template-parallel"),
                    context,
                    json!({"text":"Clear text."}),
                    cancellation,
                )
                .await
        });
        brevity.expect_delegate().await;
        clarity.expect_delegate().await;
        let (first, second, a, b) = if reverse {
            (
                &mut clarity,
                &mut brevity,
                r#"{"passed":true}"#,
                r#"{"passed":false}"#,
            )
        } else {
            (
                &mut brevity,
                &mut clarity,
                r#"{"passed":false}"#,
                r#"{"passed":true}"#,
            )
        };
        first
            .send_result(
                crate::runtime::subagent::ipc::ChildResultStatus::Succeeded,
                Some(a),
            )
            .await;
        let first_key = if reverse { "clarity" } else { "brevity" };
        loop {
            if observations.borrow().iter().any(|event| matches!(event,
                RuntimeEvent::WorkflowBlockSettled { instance, outcome: WorkflowExecutionOutcome::Completed }
                    if instance.definition.blocks == ["check_text", first_key])) { break; }
            observations.changed().await.unwrap();
        }
        second
            .send_result(
                crate::runtime::subagent::ipc::ChildResultStatus::Succeeded,
                Some(b),
            )
            .await;
        assert_eq!(
            task.await.unwrap().unwrap(),
            json!({"brevity":{"passed":false},"clarity":{"passed":true}})
        );
        assert_eq!(serde_json::to_value(frozen.inspect()).unwrap(), before);
        assert!(matches!(
            observations.borrow().last(),
            Some(RuntimeEvent::WorkflowCompleted { .. })
        ));
    }
}
