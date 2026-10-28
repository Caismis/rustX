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
