//! Final node contracts cross the native Check, Explain and execution surfaces.
use super::*;
use crate::local_runtime::configuration::settings::SourceTarget;
use crate::local_runtime::launch::{HostEnvironment, LaunchRequest};
use crate::local_runtime::workflow_authoring::{WorkflowCheckReport, WorkflowStructure};

fn checked(definition: WorkflowDefinition) -> (WorkflowProgram, WorkflowCheckReport) {
    let root = tempfile::tempdir().unwrap();
    let base = root.path().canonicalize().unwrap();
    let workspace = base.join("workspace");
    let home = base.join("home");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    let host = HostEnvironment::from_paths(workspace, home).unwrap();
    let manager = LaunchRequest::default().session_input(&host).unwrap().0;
    let id = WorkflowId::parse("test_workflow").unwrap();
    let report = manager
        .check_workflow(
            &SourceTarget::User,
            &id,
            definition.clone().into(),
            None,
            true,
        )
        .unwrap();
    (compile_test(definition).unwrap(), report)
}

fn classifier_definition() -> WorkflowDefinition {
    base_definition(
        "generate",
        BTreeMap::from([
            (
                "generate".into(),
                WorkflowNodeDefinition::Classifier {
                    model: WorkflowModelPolicy::Session,
                    input: reference("args.task"),
                    instruction: vec![text("Choose a class.")],
                    classes: vec![WorkflowClassifierClass {
                        id: "support".into(),
                        label: "Support label".into(),
                        description: vec![text("Support for "), reference("args.task")],
                    }],
                },
            ),
            (
                "done".into(),
                return_node(BTreeMap::from([("node".into(), reference("generate"))])),
            ),
        ]),
        vec![branch_edge(
            "generate",
            "done",
            WorkflowPort::case("support"),
        )],
    )
}

#[tokio::test]
async fn classifier_and_model_check_and_explain_match_committed_node_values() {
    let text_output = WorkflowModelOutput::Text {};
    let structured = WorkflowModelOutput::JsonSchema {
        schema: schema(json!({"ok":{"type":"boolean"}}), &["ok"]),
    };
    let mut cases = vec![(
        classifier_definition(),
        "classifier",
        r#"{"class_id":"support"}"#,
        crate::runtime::workflow::transform::classifier_result_schema(),
    )];
    for (output, response) in [(text_output, "model text"), (structured, r#"{"ok":true}"#)] {
        let mut definition =
            model_definition(output.clone(), crate::model::AuxiliaryModelPolicy::Session);
        definition.block.nodes.insert(
            "done".into(),
            return_node(BTreeMap::from([("node".into(), reference("generate"))])),
        );
        cases.push((definition, "model", response, output.schema()));
    }
    for (definition, kind, response, expected) in cases {
        let (program, report) = checked(definition);
        let WorkflowStructure::Valid { outputs } = &report.structure else {
            panic!("expected valid Check: {report:?}");
        };
        let contract = &outputs["block.nodes.generate"];
        assert_eq!(contract, &expected);
        assert_eq!(program.node_outputs()["block.nodes.generate"], expected);
        let explanation = &report.explanation.as_ref().unwrap().blocks["block"].nodes["generate"];
        assert_eq!(explanation.kind, kind);
        assert_eq!(explanation.output_schema.as_ref(), Some(&expected));
        let WorkflowNodeProgram::Model(model) = &program.nodes()["generate"] else {
            unreachable!()
        };
        if kind == "classifier" {
            assert_ne!(
                model.output.schema(),
                expected,
                "routing response stays an internal Model contract"
            );
            assert_eq!(
                contract["required"],
                json!(["class_id", "class_name", "class_label", "usage"])
            );
        } else {
            assert_eq!(model.output.schema(), expected);
        }
        let plane = workflow_test_plane(1);
        let (context, executor, mut entries, _) = setup(
            &plane,
            &program,
            completed(response, ModelFinishReason::Stop),
            true,
        );
        let runtime = workflow_runtime(&plane);
        let run = start(&runtime, context, workflow_cancellation().1);
        entry(&mut entries).await.release.send(()).unwrap();
        let returned = run.await.unwrap().unwrap();
        let events = plane.store.read_events(None, 128).unwrap().events;
        let committed = events
            .iter()
            .find_map(|event| match &event.event {
                RuntimeEvent::WorkflowNodeOutputRecorded {
                    instance,
                    value: Some(value),
                } if instance.node == "generate" => {
                    Some(serde_json::from_str::<Value>(value).unwrap()["value"].clone())
                }
                _ => None,
            })
            .expect("the actual node commit is recorded");
        assert_eq!(returned["node"], committed);
        jsonschema::Validator::new(contract)
            .unwrap()
            .validate(&committed)
            .unwrap();
        if kind == "classifier" {
            assert_eq!(committed["class_id"], "support");
            assert_eq!(committed["class_name"], "Support for the input");
            assert_eq!(committed["class_label"], "Support label");
            assert!(committed.get("structured_output").is_none());
        }
        assert!(entries.try_recv().is_err());
        executor.drain().await;
    }
}
