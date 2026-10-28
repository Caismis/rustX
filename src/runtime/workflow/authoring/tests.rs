use super::*;
use WorkflowReferenceAvailability::{Available, Pending, Unavailable};
use WorkflowReferenceCompatibility::{Compatible, Incompatible, Unknown};
use serde_json::json;

fn object(properties: Value, required: &[&str]) -> Value {
    field(
        json!({"type":"object", "required":required, "additionalProperties":false}),
        "properties",
        properties,
    )
}
fn definition(block: Value) -> WorkflowDefinition {
    serde_json::from_value(field(
        json!({"description":"Candidate analysis"}),
        "block",
        block,
    ))
    .unwrap()
}
fn field(mut value: Value, key: &str, content: Value) -> Value {
    value[key] = content;
    value
}
fn reference(path: &[&str]) -> Value {
    json!({"type":"reference", "path":path})
}
fn model() -> Value {
    json!({"type":"model", "instruction":"", "output":{"type":"text"}})
}
fn done(output: Value) -> Value {
    field(json!({"type":"return"}), "output", output)
}
fn query(node: &str, field: WorkflowConsumerField, path: &[&str]) -> WorkflowAnalysisQuery {
    WorkflowAnalysisQuery {
        consumer: WorkflowConsumerAddress {
            address: WorkflowNodeAddress {
                block: vec![],
                node: node.into(),
            },
            field,
            path: vec![],
        },
        search: String::new(),
        reference: Some(path.iter().map(|s| (*s).into()).collect()),
        limit: 128,
    }
}
fn selected(
    definition: &WorkflowDefinition,
    query: &WorkflowAnalysisQuery,
) -> WorkflowReferenceChoice {
    analyze(definition, Some(query)).unwrap().selected.unwrap()
}

fn selected_and_catalog(analysis: &WorkflowCandidateAnalysis) -> [&WorkflowReferenceChoice; 2] {
    let selected = analysis.selected.as_ref().unwrap();
    [
        selected,
        analysis
            .references
            .iter()
            .find(|choice| choice.path == selected.path)
            .unwrap(),
    ]
}
fn selected_and_unknown_root(
    analysis: &WorkflowCandidateAnalysis,
) -> [&WorkflowReferenceChoice; 2] {
    let selected = analysis.selected.as_ref().unwrap();
    assert!(selected.path.len() > 1);
    assert!(
        analysis
            .references
            .iter()
            .all(|choice| choice.path != selected.path)
    );
    [
        selected,
        analysis
            .references
            .iter()
            .find(|choice| choice.path == selected.path[..1])
            .unwrap(),
    ]
}
fn text_candidate() -> WorkflowDefinition {
    definition(
        json!({"input":object(json!({}), &[]), "output":object(json!({"summary":{"type":"string"}}), &["summary"]),
        "entry":"text", "nodes":{"text":model(), "done":done(json!({"type":"object", "fields":{"summary":reference(&["text"])}}))},
        "edges":[{"from":"text", "to":"done"}]}),
    )
}

#[test]
fn declarations_and_lexical_proofs_do_not_require_a_valid_program() {
    let mut candidate = text_candidate();
    assert!(
        super::super::WorkflowProgram::compile(
            super::super::WorkflowId::parse("candidate").unwrap(),
            candidate.clone()
        )
        .is_err(),
        "unfinished instruction"
    );
    let mut query = query("done", WorkflowConsumerField::ReturnOutput, &["text"]);
    query.consumer.path.push(WorkflowExpressionStep::Field {
        key: "summary".into(),
    });
    let analysis = analyze(&candidate, Some(&query)).unwrap();
    assert_eq!(analysis.outputs.len(), 1);
    assert_eq!(analysis.outputs[0].schema, Some(json!({"type":"string"})));
    assert_eq!(analysis.selected.as_ref().unwrap().availability, Available);
    assert_eq!(
        analysis.selected.as_ref().unwrap().compatibility,
        Compatible
    );
    query.reference = Some(vec!["text".into(), "text".into()]);
    assert_eq!(
        selected(&candidate, &query).availability,
        Unavailable,
        "text is a string, with no invented .text"
    );
    candidate.block.edges.clear();
    query.reference = Some(vec!["text".into()]);
    assert_eq!(selected(&candidate, &query).availability, Pending);
    assert!(analyze(&candidate, Some(&query)).unwrap().incomplete);
    assert_eq!(analyze(&candidate, None).unwrap().outputs.len(), 1);
}

#[test]
fn a_real_node_named_result_retains_its_producer_identity() {
    let mut candidate = text_candidate();
    let mut node = candidate.block.nodes.remove("text").unwrap();
    let WorkflowNodeDefinition::Model { instruction, .. } = &mut node else {
        panic!("Model")
    };
    *instruction = "Return text".into();
    candidate.block.nodes.insert("result".into(), node);
    candidate.block.entry = "result".into();
    candidate.block.edges[0].from = "result".into();
    let WorkflowNodeDefinition::Return { output } = candidate.block.nodes.get_mut("done").unwrap()
    else {
        panic!("Return")
    };
    *output = serde_json::from_value(
        json!({"type":"object","fields":{"summary":reference(&["result"])}}),
    )
    .unwrap();
    super::super::WorkflowProgram::compile(
        super::super::WorkflowId::parse("result-producer").unwrap(),
        candidate.clone(),
    )
    .unwrap();
    let mut query = query("done", WorkflowConsumerField::ReturnOutput, &["result"]);
    query.consumer.path.push(WorkflowExpressionStep::Field {
        key: "summary".into(),
    });
    let choice = selected(&candidate, &query);
    assert_eq!(choice.availability, Available);
    assert_eq!(
        choice.producer,
        Some(WorkflowNodeAddress {
            block: vec![],
            node: "result".into()
        })
    );
}

#[test]
fn actual_editor_unselected_identities_are_analyzable_but_never_compiled() {
    let cases = [
        (
            json!({"type":"agent","profile":"","task":"","input":{"value":reference(&["args"])},"output":object(json!({}),&[])}),
            WorkflowConsumerField::Input {
                binding: "value".into(),
            },
        ),
        (
            json!({"type":"model","model":{"mode":"explicit","model":"","request_params":{}},"instruction":"","input":{"value":reference(&["args"])},"output":{"type":"text"}}),
            WorkflowConsumerField::Input {
                binding: "value".into(),
            },
        ),
        (
            json!({"type":"tool","selector":{"origin":"source","source_id":"","name":"read"},"arguments":reference(&["args"]),"result":{"type":"text","part":0}}),
            WorkflowConsumerField::Arguments,
        ),
        (
            json!({"type":"tool","selector":{"origin":"builtin","name":""},"arguments":reference(&["args"]),"result":{"type":"text","part":0}}),
            WorkflowConsumerField::Arguments,
        ),
    ];
    for (node, field) in cases {
        let mut value = serde_json::to_value(text_candidate()).unwrap();
        value["block"]["nodes"]["text"] = node.clone();
        // Exactly the browser candidate spelling, without substitution or
        // removal of unselected fields before native deserialization.
        let candidate: WorkflowDefinition = serde_json::from_value(value).unwrap();
        assert_eq!(
            serde_json::to_value(&candidate).unwrap()["block"]["nodes"]["text"],
            node
        );
        let analysis = analyze(&candidate, Some(&query("text", field, &["args"]))).unwrap();
        assert!(analysis.outputs[0].schema.is_some());
        assert_eq!(analysis.selected.unwrap().availability, Available);
        assert!(
            super::super::WorkflowProgram::compile(
                super::super::WorkflowId::parse("unselected").unwrap(),
                candidate
            )
            .is_err()
        );
    }
}

#[test]
fn required_paths_whole_objects_arrays_and_type_compatibility_use_native_proofs() {
    let mut candidate = text_candidate();
    candidate.block.input = object(
        json!({
            "required":{"type":"object", "properties":{"items":{"type":"array", "items":{"type":"string"}}}, "required":["items"], "additionalProperties":false},
            "optional":{"type":"string"}, "flag":{"type":"boolean"}
        }),
        &["required", "flag"],
    );
    let mut query = query("done", WorkflowConsumerField::ReturnOutput, &["args"]);
    assert_eq!(selected(&candidate, &query).availability, Available);
    for path in [vec!["args", "required"], vec!["args", "required", "items"]] {
        query.reference = Some(path.iter().map(|s| (*s).into()).collect());
        assert_eq!(selected(&candidate, &query).availability, Available);
    }
    for path in [
        vec!["args", "optional"],
        vec!["args", "required", "items", "0"],
        vec!["no_producer"],
    ] {
        query.reference = Some(path.iter().map(|s| (*s).into()).collect());
        assert_eq!(selected(&candidate, &query).availability, Unavailable);
    }
    query.consumer.path = vec![WorkflowExpressionStep::Field {
        key: "summary".into(),
    }];
    query.reference = Some(vec!["args".into(), "flag".into()]);
    assert_eq!(selected(&candidate, &query).compatibility, Incompatible);
}

#[test]
fn joins_intersect_predecessor_values_and_never_export_branch_business_data() {
    let candidate = definition(
        json!({"input":object(json!({"flag":{"type":"boolean"}}), &["flag"]), "output":object(json!({}), &[]),
        "entry":"gate", "nodes":{
            "gate":{"type":"branch", "condition":{"type":"boolean", "value":reference(&["args","flag"])}},
            "left":model(), "right":model(), "done":done(json!({"type":"literal", "value":{}}))},
        "edges":[{"from":"gate","to":"left","port":"true"},{"from":"gate","to":"right","port":"false"},{"from":"left","to":"done"},{"from":"right","to":"done"}]}),
    );
    for producer in ["left", "right", "gate", "done"] {
        assert_eq!(
            selected(
                &candidate,
                &query("done", WorkflowConsumerField::ReturnOutput, &[producer])
            )
            .availability,
            Unavailable
        );
    }
    assert_eq!(
        selected(
            &candidate,
            &query(
                "done",
                WorkflowConsumerField::ReturnOutput,
                &["args", "flag"]
            )
        )
        .availability,
        Available
    );
    assert_eq!(analyze(&candidate, None).unwrap().outputs.len(), 2);
}

#[test]
fn parallel_projections_and_duplicate_local_ids_keep_their_exact_block_identity() {
    let child = json!({"input":object(json!({}), &[]), "output":object(json!({}), &[]), "entry":"work",
        "nodes":{"work":model(),"done":done(json!({"type":"literal","value":{}}))}, "edges":[{"from":"work","to":"done"}]});
    let candidate = definition(
        json!({"input":object(json!({}), &[]), "output":object(json!({}), &[]), "entry":"fork",
        "nodes":{"fork":{"type":"parallel","branches":{
            "body":{"input":reference(&["args"]),"block":child}, "other":{"input":reference(&["args"]),"block":child}}},
            "done":done(json!({"type":"literal","value":{}}))}, "edges":[{"from":"fork","to":"done"}]}),
    );
    let analysis = analyze(&candidate, None).unwrap();
    let work: Vec<_> = analysis
        .outputs
        .iter()
        .filter(|o| o.producer.node == "work")
        .collect();
    assert_eq!(work.len(), 2);
    assert_ne!(work[0].producer, work[1].producer);
    let mut child_query = query("done", WorkflowConsumerField::ReturnOutput, &["work"]);
    child_query.consumer.address.block = vec![WorkflowBlockStep {
        node: "fork".into(),
        branch: Some("body".into()),
    }];
    assert_eq!(selected(&candidate, &child_query).availability, Available);
    child_query.reference = Some(vec!["fork".into()]);
    assert_eq!(selected(&candidate, &child_query).availability, Unavailable);
    assert_eq!(
        selected(
            &candidate,
            &query(
                "fork",
                WorkflowConsumerField::BranchInput {
                    branch: "body".into()
                },
                &["work"]
            )
        )
        .availability,
        Unavailable
    );
    assert_eq!(
        selected(
            &candidate,
            &query(
                "fork",
                WorkflowConsumerField::BranchInput {
                    branch: "body".into()
                },
                &["args"]
            )
        )
        .availability,
        Available
    );
    assert_eq!(
        selected(
            &candidate,
            &query("done", WorkflowConsumerField::ReturnOutput, &["work"])
        )
        .availability,
        Unavailable
    );
    assert_eq!(
        selected(
            &candidate,
            &query(
                "done",
                WorkflowConsumerField::ReturnOutput,
                &["fork", "body"]
            )
        )
        .availability,
        Available
    );
    child_query.consumer.address.block[0].branch = None;
    assert!(
        analyze(&candidate, Some(&child_query))
            .unwrap()
            .diagnostic
            .is_some(),
        "Parallel body branch is distinct from Loop body"
    );
}

#[test]
fn loop_initial_body_result_and_parent_scopes_are_distinct() {
    let contract = object(json!({"ok":{"type":"boolean"}}), &["ok"]);
    let body = json!({"input":contract, "output":contract, "entry":"private",
        "nodes":{"private":model(), "done":done(reference(&["args"]))}, "edges":[{"from":"private","to":"done"}]});
    let candidate = definition(
        json!({"input":contract, "output":object(json!({}), &[]),"entry":"repeat",
        "nodes":{"repeat":{"type":"loop","input":reference(&["args"]),"body":body,"until":{"type":"boolean","value":reference(&["result","ok"])},
            "carry":reference(&["result"]),"max_iterations":2},"done":done(json!({"type":"literal","value":{}}))},
        "edges":[{"from":"repeat","to":"done","port":"satisfied"},{"from":"repeat","to":"done","port":"exhausted"}]}),
    );
    assert_eq!(
        selected(
            &candidate,
            &query("repeat", WorkflowConsumerField::LoopInput, &["args"])
        )
        .availability,
        Available
    );
    assert_eq!(
        selected(
            &candidate,
            &query("repeat", WorkflowConsumerField::LoopInput, &["result"])
        )
        .availability,
        Unavailable
    );
    let mut until = query(
        "repeat",
        WorkflowConsumerField::LoopUntil,
        &["result", "ok"],
    );
    until.consumer.path = vec![WorkflowExpressionStep::Value];
    assert_eq!(selected(&candidate, &until).availability, Available);
    assert_eq!(
        selected(&candidate, &until).producer,
        None,
        "Loop result is a synthetic lexical binding"
    );
    assert_eq!(selected(&candidate, &until).compatibility, Compatible);
    for invalid in ["args", "private"] {
        until.reference = Some(vec![invalid.into()]);
        assert_eq!(selected(&candidate, &until).availability, Unavailable);
    }
    assert_eq!(
        selected(
            &candidate,
            &query("repeat", WorkflowConsumerField::LoopCarry, &["result"])
        )
        .compatibility,
        Compatible
    );
    assert_eq!(
        selected(
            &candidate,
            &query(
                "done",
                WorkflowConsumerField::ReturnOutput,
                &["repeat", "result"]
            )
        )
        .availability,
        Available
    );
    assert_eq!(
        selected(
            &candidate,
            &query("done", WorkflowConsumerField::ReturnOutput, &["private"])
        )
        .availability,
        Unavailable
    );
}

#[test]
fn external_tool_expected_types_stay_unknown_and_bad_output_contracts_stay_pending() {
    let mut candidate = text_candidate();
    candidate.block.nodes.insert("text".into(), serde_json::from_value(json!({"type":"tool","selector":{"origin":"source","source_id":"unknown","name":"unknown"},
        "arguments":reference(&["args"]),"result":{"type":"json","part":0,"schema":{"type":"object","unknown":true}}})).unwrap());
    let args = selected(
        &candidate,
        &query("text", WorkflowConsumerField::Arguments, &["args"]),
    );
    assert_eq!(args.availability, Available);
    assert_eq!(args.compatibility, Unknown);
    let mut output_query = query("done", WorkflowConsumerField::ReturnOutput, &["text"]);
    output_query
        .consumer
        .path
        .push(WorkflowExpressionStep::Field {
            key: "summary".into(),
        });
    let report = analyze(&candidate, Some(&output_query)).unwrap();
    for reference in [
        report.selected.as_ref().unwrap(),
        report
            .references
            .iter()
            .find(|choice| choice.path == ["text"])
            .unwrap(),
    ] {
        assert_eq!(reference.availability, Pending);
        assert_eq!(reference.compatibility, Unknown);
        assert_eq!(reference.schema, None);
    }
    assert!(
        analyze(&candidate, None).unwrap().outputs[0]
            .schema
            .is_none()
    );
}

#[test]
fn unknown_output_contracts_preserve_producer_scope_facts() {
    for (schema, upstream) in [
        (json!({"type":"object","unknown":true}), Pending),
        (json!({"type":"string"}), Available),
    ] {
        let tool = || {
            json!({"type":"tool","selector":{"origin":"builtin","name":"read"},
                "arguments":reference(&["args"]),
                "result":{"type":"json","part":0,"schema":schema}})
        };
        let first = field(model(), "input", json!({"value":reference(&["broken"])}));
        let linear = definition(json!({
            "input":object(json!({}), &[]), "output":object(json!({}), &[]),
            "entry":"first", "nodes":{"first":first, "broken":tool(),
                "done":done(reference(&["args"]))},
            "edges":[{"from":"first","to":"broken"},{"from":"broken","to":"done"}]
        }));
        let joined = definition(json!({
            "input":object(json!({}), &[]), "output":object(json!({}), &[]),
            "entry":"common", "nodes":{"common":tool(),
                "gate":{"type":"branch","condition":{"type":"boolean","value":{"type":"literal","value":true}}},
                "broken":tool(), "skip":model(), "join":tool(),
                "done":done(reference(&["args"]))},
            "edges":[{"from":"common","to":"gate"},
                {"from":"gate","to":"broken","port":"true"},{"from":"gate","to":"skip","port":"false"},
                {"from":"broken","to":"join"},{"from":"skip","to":"join"},
                {"from":"join","to":"done"}]
        }));
        let cases = [
            (
                &linear,
                query("broken", WorkflowConsumerField::Arguments, &["broken"]),
                Unavailable,
            ),
            (
                &linear,
                query(
                    "first",
                    WorkflowConsumerField::Input {
                        binding: "value".into(),
                    },
                    &["broken"],
                ),
                Unavailable,
            ),
            (
                &linear,
                query("done", WorkflowConsumerField::ReturnOutput, &["broken"]),
                upstream,
            ),
            (
                &joined,
                query("join", WorkflowConsumerField::Arguments, &["broken"]),
                Unavailable,
            ),
            (
                &joined,
                query("join", WorkflowConsumerField::Arguments, &["common"]),
                upstream,
            ),
        ];
        for (candidate, query, availability) in cases {
            let report = analyze(candidate, Some(&query)).unwrap();
            let selected = report.selected.as_ref().unwrap();
            let listed = report
                .references
                .iter()
                .find(|choice| choice.path == selected.path)
                .unwrap();
            for choice in [selected, listed] {
                assert_eq!(
                    choice.availability, availability,
                    "{:?} at {:?} with {schema}",
                    choice.path, query.consumer
                );
                if upstream == Pending {
                    assert_eq!(choice.schema, None);
                    assert_eq!(choice.compatibility, Unknown);
                }
            }
        }
    }
}

#[test]
fn tool_argument_roots_enforce_necessary_object_type_without_claiming_full_compatibility() {
    for (schema, compatibility) in [
        (json!({"type":"string"}), Incompatible),
        (
            json!({"type":"array","items":{"type":"string"}}),
            Incompatible,
        ),
        (json!({"type":"boolean"}), Incompatible),
        (json!({"type":"null"}), Incompatible),
        (object(json!({}), &[]), Unknown),
    ] {
        let mut candidate = definition(json!({
            "input":object(json!({}), &[]), "output":object(json!({}), &[]), "entry":"source",
            "nodes":{
                "source":{"type":"tool","selector":{"origin":"builtin","name":"read"},"arguments":reference(&["args"]),"result":{"type":"json","part":0,"schema":schema}},
                "sink":{"type":"tool","selector":{"origin":"builtin","name":"read"},"arguments":reference(&["source"]),"result":{"type":"text","part":0}},
                "done":done(reference(&["args"]))},
            "edges":[{"from":"source","to":"sink"},{"from":"sink","to":"done"}]
        }));
        let root = query("sink", WorkflowConsumerField::Arguments, &["source"]);
        let report = analyze(&candidate, Some(&root)).unwrap();
        for choice in [
            report.selected.as_ref().unwrap(),
            report
                .references
                .iter()
                .find(|choice| choice.path == ["source"])
                .unwrap(),
        ] {
            assert_eq!(choice.availability, Available);
            assert_eq!(choice.compatibility, compatibility, "root {schema}");
            assert_eq!(choice.schema.as_ref(), Some(&schema));
        }
        let compiled = super::super::WorkflowProgram::compile(
            super::super::WorkflowId::parse("arguments").unwrap(),
            candidate.clone(),
        );
        if compatibility == Incompatible {
            assert!(
                compiled
                    .unwrap_err()
                    .to_string()
                    .contains("Tool arguments must construct an object")
            );
        } else {
            compiled.unwrap();
        }
        let WorkflowNodeDefinition::Tool { arguments, .. } =
            candidate.block.nodes.get_mut("sink").unwrap()
        else {
            panic!("Tool")
        };
        *arguments = serde_json::from_value(
            json!({"type":"object","fields":{"value":reference(&["source"])}}),
        )
        .unwrap();
        let mut nested = root;
        nested.consumer.path.push(WorkflowExpressionStep::Field {
            key: "value".into(),
        });
        let report = analyze(&candidate, Some(&nested)).unwrap();
        for choice in [
            report.selected.as_ref().unwrap(),
            report
                .references
                .iter()
                .find(|choice| choice.path == ["source"])
                .unwrap(),
        ] {
            assert_eq!(choice.availability, Available);
            assert_eq!(choice.compatibility, Unknown, "nested {schema}");
        }
        super::super::WorkflowProgram::compile(
            super::super::WorkflowId::parse("arguments").unwrap(),
            candidate,
        )
        .unwrap();
    }
}

#[test]
fn derived_aggregate_schemas_keep_native_proofs_at_the_authored_depth_boundary() {
    for depth in [31, 32] {
        let mut nested = json!({"type":"string"});
        for _ in 1..depth {
            nested = json!({"type":"array","items":nested});
        }
        let schema = object(json!({"nested":nested}), &["nested"]);
        validate_root_schema(&schema, "branch").unwrap();
        let body = json!({"input":schema,"output":schema,"entry":"done","nodes":{"done":done(reference(&["args"]))},"edges":[]});
        for (node, expected) in [
            (
                json!({"type":"parallel","branches":{"only":{"input":reference(&["args"]),"block":body}}}),
                parallel_output(serde_json::Map::from_iter([(
                    "only".into(),
                    schema.clone(),
                )])),
            ),
            (
                json!({"type":"loop","input":reference(&["args"]),"body":body,"until":{"type":"boolean","value":{"type":"literal","value":true}},"carry":reference(&["result"]),"max_iterations":1}),
                super::super::execution::loop_result_schema(&schema),
            ),
        ] {
            let mut edges = vec![
                json!({"from":"produce","to":"sink"}),
                json!({"from":"sink","to":"done"}),
            ];
            if node["type"] == "loop" {
                edges[0]["port"] = json!("satisfied");
                edges.push(json!({"from":"produce","to":"sink","port":"exhausted"}));
            }
            let candidate = definition(json!({
                "input":schema,"output":schema,"entry":"produce","nodes":{
                    "produce":node,
                    "sink":{"type":"tool","selector":{"origin":"builtin","name":"read"},"arguments":reference(&["produce"]),"result":{"type":"text","part":0}},
                    "done":done(reference(&["args"]))},
                "edges":edges
            }));
            super::super::WorkflowProgram::compile(
                super::super::WorkflowId::parse("aggregate").unwrap(),
                candidate.clone(),
            )
            .unwrap();
            assert_eq!(
                validate_workflow_schema(&expected, "authored").is_ok(),
                depth == 31
            );
            let report = analyze(
                &candidate,
                Some(&query(
                    "sink",
                    WorkflowConsumerField::Arguments,
                    &["produce"],
                )),
            )
            .unwrap();
            assert!(!report.incomplete);
            for choice in [
                report.selected.as_ref().unwrap(),
                report
                    .references
                    .iter()
                    .find(|choice| choice.path == ["produce"])
                    .unwrap(),
            ] {
                assert_eq!(choice.availability, Available);
                assert_eq!(choice.compatibility, Unknown);
                assert_eq!(choice.schema.as_ref(), Some(&expected));
            }
            assert_eq!(
                report
                    .outputs
                    .iter()
                    .find(|output| output.producer.node == "produce")
                    .unwrap()
                    .schema
                    .as_ref(),
                Some(&expected)
            );
        }
    }
}

#[test]
fn unfinished_input_contracts_preserve_control_flow_presence_and_absence() {
    let empty = object(json!({}), &[]);
    let tool = || json!({"type":"tool","selector":{"origin":"builtin","name":"read"},"arguments":reference(&["args"]),"result":{"type":"json","part":0,"schema":empty}});
    let linear = definition(
        json!({"input":empty,"output":empty,"entry":"first","nodes":{
        "first":tool(),"later":tool(),"done":done(reference(&["args"]))},
        "edges":[{"from":"first","to":"later"},{"from":"later","to":"done"}]}),
    );
    let joined = definition(
        json!({"input":empty,"output":empty,"entry":"common","nodes":{
        "common":tool(),"gate":{"type":"branch","condition":{"type":"boolean","value":{"type":"literal","value":true}}},
        "left":tool(),"right":tool(),"join":tool(),"done":done(reference(&["args"]))},
        "edges":[{"from":"common","to":"gate"},{"from":"gate","to":"left","port":"true"},{"from":"gate","to":"right","port":"false"},
            {"from":"left","to":"join"},{"from":"right","to":"join"},{"from":"join","to":"done"}]}),
    );
    for (input, known) in [
        (empty.clone(), true),
        (json!({}), false),
        (json!({"type":"object","unknown":true}), false),
        (json!({"type":"array","items":{"type":"string"}}), false),
        (Value::Null, false),
    ] {
        let mut linear = linear.clone();
        linear.block.input = input.clone();
        let mut joined = joined.clone();
        joined.block.input = input;
        for (candidate, node, producer, expected) in [
            (&linear, "later", "later", Unavailable),
            (&linear, "first", "later", Unavailable),
            (&joined, "join", "left", Unavailable),
            (&joined, "join", "common", Available),
        ] {
            let report = analyze(
                candidate,
                Some(&query(node, WorkflowConsumerField::Arguments, &[producer])),
            )
            .unwrap();
            for choice in selected_and_catalog(&report) {
                assert_eq!(choice.availability, expected);
                assert_eq!(choice.schema.as_ref(), Some(&empty));
                assert_eq!(choice.compatibility, Unknown);
            }
            assert_eq!(report.incomplete, !known);
        }
        let report = analyze(
            &joined,
            Some(&query("join", WorkflowConsumerField::Arguments, &["args"])),
        )
        .unwrap();
        for choice in selected_and_catalog(&report) {
            assert_eq!(choice.availability, if known { Available } else { Pending });
            assert_eq!(choice.schema.is_some(), known);
        }
        joined.block.edges.clear();
        assert_eq!(
            selected(
                &joined,
                &query("join", WorkflowConsumerField::Arguments, &["common"])
            )
            .availability,
            Pending
        );
    }
}

fn equality_candidate(
    schema: &Value,
    other: Value,
    operator: &str,
    loop_until: bool,
    right: bool,
) -> (WorkflowDefinition, WorkflowAnalysisQuery) {
    let root = if loop_until { "result" } else { "args" };
    let selected = reference(&[root, "candidate"]);
    let (left, right_value) = if right {
        (other, selected)
    } else {
        (selected, other)
    };
    let predicate = json!({"type":operator,"left":left,"right":right_value});
    let schema = object(json!({"candidate":schema}), &["candidate"]);
    let node = if loop_until {
        json!({"type":"loop","input":reference(&["args"]),"body":{"input":schema,"output":schema,"entry":"done","nodes":{"done":done(reference(&["args"]))},"edges":[]},
            "until":predicate,"carry":reference(&["result"]),"max_iterations":1})
    } else {
        json!({"type":"branch","condition":predicate})
    };
    let ports = if loop_until {
        ["satisfied", "exhausted"]
    } else {
        ["true", "false"]
    };
    let candidate = definition(
        json!({"input":schema,"output":schema,"entry":"gate","nodes":{"gate":node,"done":done(reference(&["args"]))},
        "edges":[{"from":"gate","to":"done","port":ports[0]},{"from":"gate","to":"done","port":ports[1]}]}),
    );
    let mut query = query(
        "gate",
        if loop_until {
            WorkflowConsumerField::LoopUntil
        } else {
            WorkflowConsumerField::Condition
        },
        &[root, "candidate"],
    );
    query.consumer.path.push(if right {
        WorkflowExpressionStep::Right
    } else {
        WorkflowExpressionStep::Left
    });
    (candidate, query)
}

#[test]
fn equality_preserves_supported_types_and_known_illegal_types_with_unknown_counterparts() {
    let samples = [
        (json!({"type":"integer"}), json!(1), false),
        (json!({"type":"number"}), json!(1.5), false),
        (
            json!({"type":"string","enum":["a","b"]}),
            json!("different"),
            true,
        ),
        (json!({"type":"boolean"}), json!(true), true),
        (json!({"type":"null"}), Value::Null, true),
        (object(json!({}), &[]), json!({}), false),
        (
            json!({"type":"array","items":{"type":"string"}}),
            json!([]),
            false,
        ),
    ];
    for operator in ["equal", "not_equal"] {
        for loop_until in [false, true] {
            for right in [false, true] {
                for (schema, literal, supported) in &samples {
                    for known_other in [false, true] {
                        let other = if known_other {
                            json!({"type":"literal","value":literal})
                        } else {
                            reference(&["unfinished"])
                        };
                        let (candidate, query) =
                            equality_candidate(schema, other, operator, loop_until, right);
                        let report = analyze(&candidate, Some(&query)).unwrap();
                        for choice in selected_and_catalog(&report) {
                            assert_eq!(choice.availability, Available);
                            assert_eq!(
                                choice.compatibility,
                                if !supported {
                                    Incompatible
                                } else if known_other {
                                    Compatible
                                } else {
                                    Unknown
                                },
                                "{operator}, loop={loop_until}, right={right}, {schema}, other={known_other}"
                            );
                            if !supported {
                                assert!(choice.diagnostic.is_some());
                            }
                        }
                        let compiled = super::super::WorkflowProgram::compile(
                            super::super::WorkflowId::parse("equality").unwrap(),
                            candidate,
                        );
                        assert_eq!(compiled.is_ok(), *supported && known_other);
                        if !supported && known_other {
                            assert!(
                                compiled.unwrap_err().to_string().contains(
                                    "equality requires matching boolean/string/null types"
                                )
                            );
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn equality_retains_invalid_counterpart_facts_and_does_not_invent_unknown_operand_types() {
    for operator in ["equal", "not_equal"] {
        for loop_until in [false, true] {
            for right in [false, true] {
                for other in [
                    json!({"type":"literal","value":1}),
                    json!({"type":"object","fields":{"unfinished":reference(&["unfinished"])}}),
                    json!({"type":"array","items":[reference(&["unfinished"])]}),
                ] {
                    let (candidate, query) = equality_candidate(
                        &json!({"type":"string"}),
                        other,
                        operator,
                        loop_until,
                        right,
                    );
                    for unknown_selected in [false, true] {
                        let mut candidate = candidate.clone();
                        if unknown_selected {
                            make_equality_selected_type_unknown(&mut candidate, loop_until);
                        }
                        let report = analyze(&candidate, Some(&query)).unwrap();
                        let choices = if unknown_selected {
                            selected_and_unknown_root(&report)
                        } else {
                            selected_and_catalog(&report)
                        };
                        for choice in choices {
                            assert_eq!(choice.compatibility, Incompatible);
                            assert!(
                                choice
                                    .diagnostic
                                    .as_ref()
                                    .unwrap()
                                    .contains("native type constraints")
                            );
                        }
                    }
                }
                let (candidate, query) = equality_candidate(
                    &json!({"type":"string"}),
                    json!({"type":"literal","value":false}),
                    operator,
                    loop_until,
                    right,
                );
                for choice in selected_and_catalog(&analyze(&candidate, Some(&query)).unwrap()) {
                    assert_eq!(choice.compatibility, Incompatible);
                }
                for other in [
                    json!({"type":"literal","value":"known"}),
                    reference(&["unfinished"]),
                ] {
                    let (mut candidate, query) = equality_candidate(
                        &json!({"type":"string"}),
                        other,
                        operator,
                        loop_until,
                        right,
                    );
                    make_equality_selected_type_unknown(&mut candidate, loop_until);
                    for choice in
                        selected_and_unknown_root(&analyze(&candidate, Some(&query)).unwrap())
                    {
                        assert_eq!(choice.availability, Pending);
                        assert_eq!(choice.schema, None);
                        assert_eq!(choice.compatibility, Unknown);
                    }
                }
            }
        }
    }
}

fn make_equality_selected_type_unknown(candidate: &mut WorkflowDefinition, loop_until: bool) {
    if loop_until {
        let WorkflowNodeDefinition::Loop { body, .. } =
            candidate.block.nodes.get_mut("gate").unwrap()
        else {
            panic!("Loop")
        };
        body.output = json!({});
    } else {
        candidate.block.input = json!({});
    }
}

fn field_projection_candidate(
    destination: &Value,
    receiver: &str,
    shape: &str,
) -> (WorkflowDefinition, WorkflowAnalysisQuery) {
    let root = if receiver == "carry" {
        "result"
    } else {
        "args"
    };
    let value = reference(&[root, "x"]);
    let (value, steps) = match shape {
        "object" => (
            json!({"type":"object","fields":{"leaf":value}}),
            vec![WorkflowExpressionStep::Field { key: "leaf".into() }],
        ),
        "array" => (
            json!({"type":"array","items":[{"type":"object","fields":{"leaf":value}}]}),
            vec![
                WorkflowExpressionStep::Item { index: 0 },
                WorkflowExpressionStep::Field { key: "leaf".into() },
            ],
        ),
        _ => (value, vec![]),
    };
    let value = json!({"type":"object","fields":{"x":value}});
    let empty = object(json!({}), &[]);
    let source = object(json!({"x":{"type":"string"}}), &["x"]);
    let (node, field, output) = match receiver {
        "return" => (
            done(value),
            WorkflowConsumerField::ReturnOutput,
            destination.clone(),
        ),
        "parallel" => (
            json!({"type":"parallel","branches":{"only":{"input":value,"block":{"input":destination,"output":empty,"entry":"done","nodes":{"done":done(json!({"type":"literal","value":{}}))},"edges":[]}}}}),
            WorkflowConsumerField::BranchInput {
                branch: "only".into(),
            },
            empty.clone(),
        ),
        _ => {
            let carry = receiver == "carry";
            let body_output = if carry { source.clone() } else { empty.clone() };
            let body_value = if carry {
                json!({"x":"known"})
            } else {
                json!({})
            };
            (
                json!({"type":"loop","input":if carry { json!({"type":"literal","value":{}}) } else { value.clone() },"body":{"input":destination,"output":body_output,"entry":"done","nodes":{"done":done(json!({"type":"literal","value":body_value}))},"edges":[]},"until":{"type":"boolean","value":{"type":"literal","value":true}},"carry":if carry { value } else { json!({"type":"literal","value":{}}) },"max_iterations":1}),
                if carry {
                    WorkflowConsumerField::LoopCarry
                } else {
                    WorkflowConsumerField::LoopInput
                },
                empty.clone(),
            )
        }
    };
    let candidate = if receiver == "return" {
        definition(
            json!({"input":source,"output":output,"entry":"receive","nodes":{"receive":node},"edges":[]}),
        )
    } else {
        let edges = if receiver == "parallel" {
            json!([{"from":"receive","to":"done"}])
        } else {
            json!([{"from":"receive","to":"done","port":"satisfied"},{"from":"receive","to":"done","port":"exhausted"}])
        };
        definition(
            json!({"input":source,"output":output,"entry":"receive","nodes":{"receive":node,"done":done(json!({"type":"literal","value":{}}))},"edges":edges}),
        )
    };
    let mut query = query("receive", field, &[root, "x"]);
    query
        .consumer
        .path
        .push(WorkflowExpressionStep::Field { key: "x".into() });
    query.consumer.path.extend(steps);
    (candidate, query)
}

#[test]
fn closed_destination_fields_reject_references_across_receivers_and_descendants() {
    for receiver in ["return", "parallel", "input", "carry"] {
        for shape in ["scalar", "object", "array"] {
            let property = match shape {
                "object" => object(json!({"leaf":{"type":"string"}}), &["leaf"]),
                "array" => {
                    json!({"type":"array","items":object(json!({"leaf":{"type":"string"}}), &["leaf"])})
                }
                _ => json!({"type":"string"}),
            };
            for (destination, compatibility, valid) in [
                (object(json!({}), &[]), Incompatible, false),
                (
                    json!({"type":"object","additionalProperties":true}),
                    Unknown,
                    true,
                ),
                (object(json!({"x":property}), &[]), Compatible, true),
                (
                    json!({"type":"object","additionalProperties":false,"unknown":true}),
                    Unknown,
                    false,
                ),
            ] {
                let (candidate, query) = field_projection_candidate(&destination, receiver, shape);
                let report = analyze(&candidate, Some(&query)).unwrap();
                for choice in selected_and_catalog(&report) {
                    assert_eq!(choice.availability, Available);
                    assert_eq!(
                        choice.compatibility, compatibility,
                        "{receiver}, {shape}, {destination}"
                    );
                    if compatibility == Incompatible {
                        assert!(
                            choice
                                .diagnostic
                                .as_ref()
                                .unwrap()
                                .contains("additionalProperties: false")
                        );
                    }
                }
                assert_eq!(
                    super::super::WorkflowProgram::compile(
                        super::super::WorkflowId::parse("projection").unwrap(),
                        candidate
                    )
                    .is_ok(),
                    valid,
                    "{receiver}, {shape}, {destination}"
                );
            }
        }
    }
}

#[test]
fn forbidden_destination_fields_do_not_require_known_producer_types() {
    for receiver in ["return", "parallel", "input", "carry"] {
        for shape in ["scalar", "object", "array"] {
            let (mut candidate, query) =
                field_projection_candidate(&object(json!({}), &[]), receiver, shape);
            if receiver == "carry" {
                let WorkflowNodeDefinition::Loop { body, .. } =
                    candidate.block.nodes.get_mut("receive").unwrap()
                else {
                    panic!("Loop")
                };
                body.output = json!({});
            } else {
                candidate.block.input = json!({});
            }
            let report = analyze(&candidate, Some(&query)).unwrap();
            for choice in selected_and_unknown_root(&report) {
                assert_eq!(choice.availability, Pending);
                assert_eq!(choice.schema, None);
                assert_eq!(choice.compatibility, Incompatible);
                assert!(
                    choice
                        .diagnostic
                        .as_ref()
                        .unwrap()
                        .contains("destination schema forbids this field")
                );
            }
        }
    }
}

#[test]
fn analysis_bounds_search_and_truncation_are_explicit() {
    let mut candidate = text_candidate();
    candidate.block.input = object(
        Value::Object(
            (0..300)
                .map(|i| {
                    (
                        format!("field_{i}"),
                        json!({"type":"string","description":"x".repeat(512)}),
                    )
                })
                .collect(),
        ),
        &[],
    );
    let mut query = query("done", WorkflowConsumerField::ReturnOutput, &["args"]);
    query.limit = 3;
    let result = analyze(&candidate, Some(&query)).unwrap();
    assert!(result.truncated);
    assert_eq!(result.references.len(), 3);
    assert!(serde_json::to_vec(&result).unwrap().len() < MAX_ANALYSIS_BYTES);
    query.search = "field_299".into();
    assert_eq!(
        analyze(&candidate, Some(&query)).unwrap().references.len(),
        1
    );
    query.limit = MAX_ANALYSIS_CHOICES + 1;
    assert!(analyze(&candidate, Some(&query)).is_err());
}
