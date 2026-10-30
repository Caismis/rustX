use super::*;
use WorkflowReferenceAvailability::{Available, Pending, Unavailable};
use WorkflowReferenceCompatibility::{Compatible, Incompatible, Unknown};
use serde_json::json;

// Metadata-free fixtures exercise the same production analysis entry point.
fn analyze(
    definition: &WorkflowDefinition,
    query: Option<&WorkflowAnalysisQuery>,
) -> Result<WorkflowCandidateAnalysis, &'static str> {
    super::analyze(definition, query, &BTreeSet::new())
}
fn analyze_with_schema_budget(
    definition: &WorkflowDefinition,
    query: Option<&WorkflowAnalysisQuery>,
    budget: SchemaBudget,
) -> Result<WorkflowCandidateAnalysis, &'static str> {
    super::analyze_with_schema_budget(definition, query, &BTreeSet::new(), budget)
}

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
    json!({"type":"model", "messages":[{"role":"system","content":[]}], "output":{"type":"text"}})
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
        json!({"input":object(json!({}), &[]), "output":object(json!({"summary":{"type":"string"}}), &["summary"]),"nodes": {"workflow_start":{"type":"start"},"text":model(), "done":done(json!({"type":"object", "fields":{"summary":reference(&["text", "text"])}}))},"edges": [{"from":"workflow_start","to":"text"},{"from":"text", "to":"done"}],}),
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
        "unfinished messages"
    );
    let mut query = query(
        "done",
        WorkflowConsumerField::ReturnOutput,
        &["text", "text"],
    );
    query.consumer.path.push(WorkflowExpressionStep::Field {
        key: "summary".into(),
    });
    let analysis = analyze(&candidate, Some(&query)).unwrap();
    assert_eq!(analysis.outputs.len(), 1);
    assert_eq!(
        analysis.outputs[0].schema,
        Some(super::super::WorkflowModelOutput::Text {}.schema())
    );
    assert_eq!(analysis.selected.as_ref().unwrap().availability, Available);
    assert_eq!(
        analysis.selected.as_ref().unwrap().compatibility,
        Compatible
    );
    query.reference = Some(vec!["text".into(), "text".into(), "text".into()]);
    assert_eq!(
        selected(&candidate, &query).availability,
        Unavailable,
        "text.text is a string, with no invented .text"
    );
    candidate.block.edges.clear();
    query.reference = Some(vec!["text".into(), "text".into()]);
    assert_eq!(selected(&candidate, &query).availability, Pending);
    assert!(analyze(&candidate, Some(&query)).unwrap().incomplete);
    assert_eq!(analyze(&candidate, None).unwrap().outputs.len(), 1);
}

#[test]
fn a_real_node_named_result_retains_its_producer_identity() {
    let mut candidate = text_candidate();
    let mut node = candidate.block.nodes.remove("text").unwrap();
    let WorkflowNodeDefinition::Model { messages, .. } = &mut node else {
        panic!("Model")
    };
    *messages = serde_json::from_value(json!([{"role":"system","content":[{"type":"literal","value":"Return text"}]},{"role":"user","content":[{"type":"literal","value":"Go."}]}])).unwrap();
    candidate.block.nodes.insert("result".into(), node);
    for edge in &mut candidate.block.edges {
        if edge.from == "text" {
            edge.from = "result".into();
        }
        if edge.to == "text" {
            edge.to = "result".into();
        }
    }
    let WorkflowNodeDefinition::Return { output } = candidate.block.nodes.get_mut("done").unwrap()
    else {
        panic!("Return")
    };
    *output = serde_json::from_value(
        json!({"type":"object","fields":{"summary":reference(&["result", "text"])}}),
    )
    .unwrap();
    super::super::WorkflowProgram::compile(
        super::super::WorkflowId::parse("result-producer").unwrap(),
        candidate.clone(),
    )
    .unwrap();
    let mut query = query(
        "done",
        WorkflowConsumerField::ReturnOutput,
        &["result", "text"],
    );
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
            json!({"type":"agent","profile":"","task":[reference(&["args"])],"output":{"type":"json_schema","schema":object(json!({}),&[])}}),
            WorkflowConsumerField::Task,
        ),
        (
            json!({"type":"model","model":{"mode":"explicit","model":"","request_params":{}},"messages":[{"role":"system","content":[]},{"role":"user","content":[reference(&["args"])]}],"output":{"type":"text"}}),
            WorkflowConsumerField::Message { index: 1 },
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

fn finite_join_candidate(
    contract: &Value,
    producer: &str,
    joined: bool,
    tool_consumer: bool,
) -> (WorkflowDefinition, WorkflowConsumerField) {
    let tool = |schema: &Value, arguments: Value| {
        json!({"type":"tool", "selector":{"origin":"builtin","name":"read"},
            "arguments":arguments, "result":{"type":"json","part":0,"schema":schema}})
    };
    let mut nodes = json!({
        "source":tool(contract, json!({"type":"literal","value":{}}))
    });
    let mut edges = vec![];
    if joined {
        nodes["gate"] = json!({"type":"branch","cases": [{"id":"true", "condition":{"type":"boolean",
            "value":{"type":"literal","value":true}}}]});
        for side in ["left", "right"] {
            nodes[side] = field(
                model(),
                "messages",
                json!([{"role":"system","content":[{"type":"literal","value":"Return text"}]},{"role":"user","content":[{"type":"literal","value":"Go."}]}]),
            );
            edges.push(json!({"from":"gate","to":side,
                "port":if side == "left" { "true" } else { "false" }}));
            edges.push(json!({"from":side,"to":"sink"}));
        }
        edges.push(json!({"from":"source","to":"gate"}));
    } else {
        edges.push(json!({"from":"source","to":"sink"}));
    }
    let consumer_field = if tool_consumer {
        nodes["sink"] = tool(&json!({"type":"object"}), reference(&[producer]));
        nodes["done"] = done(json!({"type":"literal","value":{}}));
        edges.push(json!({"from":"sink","to":"done"}));
        WorkflowConsumerField::Arguments
    } else {
        nodes["sink"] = done(reference(&[producer]));
        WorkflowConsumerField::ReturnOutput
    };
    let candidate = definition(
        json!({"input":contract, "output":{"type":"object"},"nodes": ({ let mut nodes = serde_json::to_value(nodes).unwrap(); nodes.as_object_mut().unwrap().insert("workflow_start".into(), serde_json::json!({"type":"start"})); nodes }),"edges": ({ let mut edges = serde_json::to_value(edges).unwrap(); edges.as_array_mut().unwrap().insert(0, serde_json::json!({"from":"workflow_start","to":"source"})); edges }),}),
    );
    (candidate, consumer_field)
}

#[test]
fn joins_preserve_producer_identity_independently_of_finite_contract_assignability() {
    for values in [
        json!([{"x":"ok"}, {"x":"also ok"}]),
        json!([{"x":"ok"}, {}]),
        json!([{"x":"ok"}, {"x":1}]),
    ] {
        let contract = json!({"type":"object", "properties":{"x":{"type":"string"}},
            "required":["x"], "enum":values});
        for producer in ["source", "args"] {
            for joined in [false, true] {
                for tool_consumer in [false, true] {
                    let (candidate, consumer_field) =
                        finite_join_candidate(&contract, producer, joined, tool_consumer);
                    super::super::WorkflowProgram::compile(
                        super::super::WorkflowId::parse("finite-join").unwrap(),
                        candidate.clone(),
                    )
                    .unwrap();
                    let report = analyze(
                        &candidate,
                        Some(&query("sink", consumer_field, &[producer])),
                    )
                    .unwrap();
                    assert!(!report.incomplete && !report.truncated);
                    for choice in selected_and_catalog(&report) {
                        assert_eq!(choice.availability, Available);
                        assert_eq!(choice.schema.as_ref(), Some(&contract));
                        assert_eq!(
                            choice.compatibility,
                            if tool_consumer { Unknown } else { Compatible }
                        );
                        assert!(choice.diagnostic.is_none());
                    }

                    // Scope presence does not establish type assignability.
                    // A narrower consumer still rejects a mismatching value.
                    if !tool_consumer {
                        let mut incompatible = candidate.clone();
                        incompatible.block.output =
                            object(json!({"other":{"type":"string"}}), &["other"]);
                        let report = analyze(
                            &incompatible,
                            Some(&query(
                                "sink",
                                WorkflowConsumerField::ReturnOutput,
                                &[producer],
                            )),
                        )
                        .unwrap();
                        for choice in selected_and_catalog(&report) {
                            assert_eq!(choice.availability, Available);
                            assert_eq!(choice.compatibility, Incompatible);
                        }
                        assert!(
                            super::super::WorkflowProgram::compile(
                                super::super::WorkflowId::parse("finite-join-mismatch").unwrap(),
                                incompatible,
                            )
                            .is_err()
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn joins_intersect_predecessor_values_and_never_export_branch_business_data() {
    let candidate = definition(
        json!({"input":object(json!({"flag":{"type":"boolean"}}), &["flag"]), "output":object(json!({}), &[]),"nodes": {"workflow_start":{"type":"start"},
            "gate":{"type":"branch","cases": [{"id":"true", "condition":{"type":"boolean", "value":reference(&["args","flag"])}}]},
            "left":model(), "right":model(), "done":done(json!({"type":"literal", "value":{}}))},"edges": [{"from":"workflow_start","to":"gate"},{"from":"gate","to":"left","port":"true"},{"from":"gate","to":"right","port":"false"},{"from":"left","to":"done"},{"from":"right","to":"done"}],}),
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
fn loop_initial_body_result_and_parent_scopes_are_distinct() {
    let contract = object(json!({"ok":{"type":"boolean"}}), &["ok"]);
    let body = json!({"input":contract, "output":contract,"nodes": {"workflow_start":{"type":"start"},"private":model(), "done":done(reference(&["args"]))},"edges": [{"from":"workflow_start","to":"private"},{"from":"private","to":"done"}],});
    let candidate = definition(
        json!({"input":contract, "output":object(json!({}), &[]),"nodes": {"workflow_start":{"type":"start"},"repeat":{"type":"loop","input":reference(&["args"]),"body":body,"until":{"type":"boolean","value":reference(&["result","ok"])},
            "carry":reference(&["result"]),"max_iterations":2},"done":done(json!({"type":"literal","value":{}}))},"edges": [{"from":"workflow_start","to":"repeat"},{"from":"repeat","to":"done","port":"satisfied"},{"from":"repeat","to":"done","port":"exhausted"}],}),
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
        let first = field(
            model(),
            "messages",
            json!([{"role":"system","content":[]},{"role":"user","content":[reference(&["broken"])]}]),
        );
        let linear = definition(json!({
            "input":object(json!({}), &[]), "output":object(json!({}), &[]),"nodes": {"workflow_start":{"type":"start"},"first":first, "broken":tool(),
                "done":done(reference(&["args"]))},"edges": [{"from":"workflow_start","to":"first"},{"from":"first","to":"broken"},{"from":"broken","to":"done"}],}));
        let joined = definition(json!({
            "input":object(json!({}), &[]), "output":object(json!({}), &[]),"nodes": {"workflow_start":{"type":"start"},"common":tool(),
                "gate":{"type":"branch","cases": [{"id":"true", "condition":{"type":"boolean","value":{"type":"literal","value":true}}}]},
                "broken":tool(), "skip":model(), "join":tool(),
                "done":done(reference(&["args"]))},"edges": [{"from":"workflow_start","to":"common"},{"from":"common","to":"gate"},
                {"from":"gate","to":"broken","port":"true"},{"from":"gate","to":"skip","port":"false"},
                {"from":"broken","to":"join"},{"from":"skip","to":"join"},
                {"from":"join","to":"done"}],}));
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
                    WorkflowConsumerField::Message { index: 1 },
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
            "input":object(json!({}), &[]), "output":object(json!({}), &[]),"nodes": {"workflow_start":{"type":"start"},
                "source":{"type":"tool","selector":{"origin":"builtin","name":"read"},"arguments":reference(&["args"]),"result":{"type":"json","part":0,"schema":schema}},
                "sink":{"type":"tool","selector":{"origin":"builtin","name":"read"},"arguments":reference(&["source"]),"result":{"type":"text","part":0}},
                "done":done(reference(&["args"]))},"edges": [{"from":"workflow_start","to":"source"},{"from":"source","to":"sink"},{"from":"sink","to":"done"}],}));
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
        let body = json!({"input":schema,"output":schema,"nodes": {"workflow_start":{"type":"start"},"done":done(reference(&["args"]))},"edges": [{"from":"workflow_start","to":"done"}],});
        for (node, expected) in [(
            json!({"type":"loop","input":reference(&["args"]),"body":body,"until":{"type":"boolean","value":{"type":"literal","value":true}},"carry":reference(&["result"]),"max_iterations":1}),
            super::super::execution::loop_result_schema(&schema),
        )] {
            let mut edges = vec![
                json!({"from":"produce","to":"sink"}),
                json!({"from":"sink","to":"done"}),
            ];
            if node["type"] == "loop" {
                edges[0]["port"] = json!("satisfied");
                edges.push(json!({"from":"produce","to":"sink","port":"exhausted"}));
            }
            let candidate = definition(json!({
                "input":schema,"output":schema,"nodes": {"workflow_start":{"type":"start"},
                    "produce":node,
                    "sink":{"type":"tool","selector":{"origin":"builtin","name":"read"},"arguments":reference(&["produce"]),"result":{"type":"text","part":0}},
                    "done":done(reference(&["args"]))},"edges": ({ let mut edges = serde_json::to_value(edges).unwrap(); edges.as_array_mut().unwrap().insert(0, serde_json::json!({"from":"workflow_start","to":"produce"})); edges }),}));
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
        json!({"input":empty,"output":empty,"nodes": {"workflow_start":{"type":"start"},
        "first":tool(),"later":tool(),"done":done(reference(&["args"]))},"edges": [{"from":"workflow_start","to":"first"},{"from":"first","to":"later"},{"from":"later","to":"done"}],}),
    );
    let joined = definition(
        json!({"input":empty,"output":empty,"nodes": {"workflow_start":{"type":"start"},
        "common":tool(),"gate":{"type":"branch","cases": [{"id":"true", "condition":{"type":"boolean","value":{"type":"literal","value":true}}}]},
        "left":tool(),"right":tool(),"join":tool(),"done":done(reference(&["args"]))},"edges": [{"from":"workflow_start","to":"common"},{"from":"common","to":"gate"},{"from":"gate","to":"left","port":"true"},{"from":"gate","to":"right","port":"false"},
            {"from":"left","to":"join"},{"from":"right","to":"join"},{"from":"join","to":"done"}],}),
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
        json!({"type":"loop","input":reference(&["args"]),"body":{"input":schema,"output":schema,"nodes": {"workflow_start":{"type":"start"},"done":done(reference(&["args"]))},"edges": [{"from":"workflow_start","to":"done"}],},
            "until":predicate,"carry":reference(&["result"]),"max_iterations":1})
    } else {
        json!({"type":"branch","cases": [{"id":"true", "condition":predicate}]})
    };
    let ports = if loop_until {
        ["satisfied", "exhausted"]
    } else {
        ["true", "false"]
    };
    let candidate = definition(
        json!({"input":schema,"output":schema,"nodes": {"workflow_start":{"type":"start"},"gate":node,"done":done(reference(&["args"]))},"edges": [{"from":"workflow_start","to":"gate"},{"from":"gate","to":"done","port":ports[0]},{"from":"gate","to":"done","port":ports[1]}],}),
    );
    let mut query = query(
        "gate",
        if loop_until {
            WorkflowConsumerField::LoopUntil
        } else {
            WorkflowConsumerField::Condition {
                case_id: "true".into(),
            }
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
fn branch_variable_choices_ignore_the_old_counterpart_but_keep_scalar_necessities() {
    for (kind, expected) in [
        ("string", Unknown),
        ("boolean", Unknown),
        ("null", Unknown),
        ("number", Incompatible),
        ("integer", Incompatible),
    ] {
        let (candidate, mut query) = equality_candidate(
            &json!({"type":kind}),
            json!({"type":"literal","value":42}),
            "equal",
            false,
            false,
        );
        query.consumer.field = WorkflowConsumerField::ConditionVariable {
            case_id: "true".into(),
        };
        for choice in selected_and_catalog(&analyze(&candidate, Some(&query)).unwrap()) {
            assert_eq!(choice.availability, Available);
            assert_eq!(choice.compatibility, expected, "{kind}");
        }
        query.consumer.field = WorkflowConsumerField::ConditionVariable {
            case_id: "missing".into(),
        };
        assert!(
            analyze(&candidate, Some(&query))
                .unwrap()
                .diagnostic
                .is_some()
        );
    }
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

#[test]
fn closed_destination_fields_reject_references_across_receivers_and_descendants() {
    for receiver in ["return", "input", "carry"] {
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
    for receiver in ["return", "input", "carry"] {
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

fn assert_array_choice(
    candidate: &WorkflowDefinition,
    query: &WorkflowAnalysisQuery,
    compatibility: WorkflowReferenceCompatibility,
    valid_after: bool,
) {
    let report = analyze(candidate, Some(query)).unwrap();
    for choice in selected_and_catalog(&report) {
        assert_eq!(choice.availability, Available);
        assert_eq!(choice.compatibility, compatibility, "{:?}", query.reference);
        if compatibility == Incompatible {
            assert!(
                choice
                    .diagnostic
                    .as_ref()
                    .is_some_and(|reason| reason.contains("constructed array"))
            );
        }
    }
    let mut changed = candidate.clone();
    replace_array_reference(&mut changed, query);
    let checked = super::super::WorkflowProgram::compile(
        super::super::WorkflowId::parse("array").unwrap(),
        changed,
    );
    assert_eq!(checked.is_ok(), valid_after, "{:?}", checked.as_ref().err());
}

#[test]
fn array_choices_share_native_common_element_proofs_for_every_receiver() {
    for receiver in ["return", "input", "carry", "tool"] {
        let root = if receiver == "carry" {
            "result"
        } else {
            "args"
        };
        let (candidate, mut query) = array_reference_candidate(
            receiver,
            &[reference(&[root, "a"]), reference(&[root, "a"])],
            &json!({"type":"object"}),
        );
        assert!(
            super::super::WorkflowProgram::compile(
                super::super::WorkflowId::parse("array").unwrap(),
                candidate.clone()
            )
            .is_ok()
        );
        for index in [0, 1] {
            query.consumer.path[1] = WorkflowExpressionStep::Item { index };
            for field in ["a", "b", "open"] {
                query.reference = Some(vec![root.into(), field.into()]);
                assert_array_choice(
                    &candidate,
                    &query,
                    if field == "b" {
                        Incompatible
                    } else if receiver == "tool" {
                        Unknown
                    } else {
                        Compatible
                    },
                    field != "b",
                );
            }
        }
    }
}

#[test]
fn array_choices_preserve_finite_values_widening_unknown_siblings_and_repair() {
    let object_type = json!({"type":"object"});
    let a = reference(&["args", "a"]);
    let (candidate, mut query) = array_reference_candidate(
        "return",
        &[a.clone(), a.clone(), reference(&["args", "open"])],
        &object_type,
    );
    query.reference = Some(vec!["args".into(), "b".into()]);
    assert_array_choice(&candidate, &query, Compatible, true);

    for other in [
        reference(&["unfinished"]),
        json!({"type":"object","fields":{"pending":reference(&["unfinished"])}}),
    ] {
        let (candidate, mut query) =
            array_reference_candidate("return", &[other, a.clone()], &object_type);
        query.reference = Some(vec!["args".into(), "b".into()]);
        assert_array_choice(&candidate, &query, Unknown, false);
    }
    let (candidate, query) = array_reference_candidate(
        "return",
        &[a.clone(), reference(&["unfinished"])],
        &object_type,
    );
    assert_array_choice(&candidate, &query, Compatible, true);

    for (left, right, element, expected) in [
        (
            json!({"type":"object","const":{"left":"a"}}),
            json!({"type":"object","const":{"right":"b"}}),
            object_type,
            Compatible,
        ),
        (
            json!({"type":"string","const":"a"}),
            json!({"type":"string","const":"b"}),
            json!({"type":"string"}),
            Compatible,
        ),
        (
            json!({"type":"string","enum":["a"]}),
            json!({"type":"string","enum":["b"]}),
            json!({"type":"string"}),
            Incompatible,
        ),
        (
            json!({"type":"integer"}),
            json!({"type":"number"}),
            json!({"type":"number"}),
            Compatible,
        ),
        (
            json!({"type":"null","const":null}),
            json!({"type":"null","const":null}),
            json!({"type":"null"}),
            Compatible,
        ),
    ] {
        let (mut candidate, mut query) =
            array_reference_candidate("return", &[a.clone(), a.clone()], &element);
        candidate.block.input = object(json!({"a":left,"b":right}), &["a", "b"]);
        query.reference = Some(vec!["args".into(), "b".into()]);
        assert_array_choice(&candidate, &query, expected, expected == Compatible);
    }
}

#[test]
fn nested_array_choices_replace_only_the_selected_descendant_and_keep_outer_constraints() {
    let a = reference(&["args", "a"]);
    let element = json!({"type":"object"});
    let nested = json!({"type":"object","fields":{"slot":a}});
    let inner = json!({"type":"array","items":[a,a]});
    for (items, item_schema, steps) in [
        (
            vec![nested.clone(), nested],
            object(json!({"slot":element}), &["slot"]),
            vec![WorkflowExpressionStep::Field { key: "slot".into() }],
        ),
        (
            vec![inner.clone(), inner],
            json!({"type":"array","items":element}),
            vec![WorkflowExpressionStep::Item { index: 1 }],
        ),
    ] {
        let (candidate, mut query) = array_reference_candidate("return", &items, &item_schema);
        query.consumer.path.extend(steps);
        for field in ["a", "b", "open"] {
            query.reference = Some(vec!["args".into(), field.into()]);
            assert_array_choice(
                &candidate,
                &query,
                if field == "b" {
                    Incompatible
                } else {
                    Compatible
                },
                field != "b",
            );
        }
    }
}

#[test]
fn array_schema_markers_stay_unknown_and_large_repeated_arrays_keep_bounded_searches() {
    let a = reference(&["args", "a"]);
    let (mut candidate, mut query) = array_reference_candidate(
        "return",
        &[reference(&["unfinished"]), a.clone()],
        &json!({"type":"object"}),
    );
    candidate.block.nodes.insert("unfinished".into(), serde_json::from_value(json!({"type":"tool","selector":{"origin":"builtin","name":"read"},"arguments":reference(&["args"]),"result":{"type":"json","part":0,"schema":{"type":"object","unknown":true}}})).unwrap());
    candidate.block.edges = serde_json::from_value(json!([
        {"from":"workflow_start","to":"unfinished"},
        {"from":"unfinished","to":"receive"}
    ]))
    .unwrap();
    query.reference = Some(vec!["args".into(), "b".into()]);
    assert_array_choice(&candidate, &query, Unknown, false);
    query.reference = Some(vec!["unfinished".into()]);
    for choice in selected_and_catalog(&analyze(&candidate, Some(&query)).unwrap()) {
        assert_eq!(choice.availability, Pending);
        assert_eq!(choice.schema, None);
        assert_eq!(choice.compatibility, Unknown);
    }

    let (mut candidate, mut query) =
        array_reference_candidate("return", &vec![a; 1500], &json!({"type":"object"}));
    let right = candidate.block.input["properties"]["b"].clone();
    for index in 0..64 {
        let key = format!("bad_{index:03}");
        candidate.block.input["properties"][&key] = right.clone();
        candidate.block.input["required"]
            .as_array_mut()
            .unwrap()
            .push(json!(key));
    }
    query.consumer.path[1] = WorkflowExpressionStep::Item { index: 1499 };
    query.reference = Some(vec!["args".into(), "b".into()]);
    query.limit = 256;
    let report = analyze(&candidate, Some(&query)).unwrap();
    assert_eq!(
        report.selected.as_ref().unwrap().compatibility,
        Incompatible
    );
    let choices: Vec<_> = report
        .references
        .iter()
        .filter(|choice| choice.path.len() == 2 && choice.path[1].starts_with("bad_"))
        .collect();
    assert_eq!(choices.len(), 64);
    assert!(
        choices
            .iter()
            .all(|choice| choice.compatibility == Incompatible)
    );
    assert_array_choice(&candidate, &query, Incompatible, false);
}

#[test]
fn large_repeated_array_schemas_are_borrowed_before_any_proof_or_copy() {
    let (mut candidate, mut query) = array_reference_candidate(
        "tool",
        &vec![reference(&["args"]); 1500],
        &json!({"type":"object"}),
    );
    let keys: Vec<_> = (0..100).map(|index| format!("p{index:03}")).collect();
    candidate.block.input = object(
        Value::Object(
            keys.iter()
                .map(|key| {
                    (
                        key.clone(),
                        json!({"type":"string","description":"x".repeat(4096)}),
                    )
                })
                .collect(),
        ),
        &keys.iter().map(String::as_str).collect::<Vec<_>>(),
    );
    assert!(serde_json::to_vec(&candidate).unwrap().len() < super::super::MAX_WORKFLOW_BYTES);
    let WorkflowNodeDefinition::Tool { arguments, .. } = &candidate.block.nodes["receive"] else {
        panic!("Tool")
    };
    let WorkflowValue::Object { fields } = arguments else {
        panic!("object")
    };
    let WorkflowValue::Array { items } = &fields["values"] else {
        panic!("array")
    };
    let declarations = SchemaMap::from_schema(&candidate.block.input);
    let mut inference = SchemaInference::new(&declarations, "receive", SchemaBudget::default());
    let siblings = inference.siblings(items, 1499).unwrap();
    assert_eq!(siblings.len(), 1);
    assert!(
        matches!(&siblings[0], Schema::Borrowed(schema, _) if std::ptr::eq(std::ptr::from_ref(*schema), std::ptr::from_ref(&declarations.0["args"])))
    );
    // Only handles and one measurement entry, independent of description bytes
    // times the number of references. No schema tree has been materialized.
    assert!(inference.budget.allocation_used < 512 * 1024);
    assert!(!inference.budget.exhausted);
    query.consumer.path[1] = WorkflowExpressionStep::Item { index: 1499 };
    query.reference = Some(vec!["args".into(), "p000".into()]);
    query.search = "args.p000".into();
    for limit in [1, 128] {
        query.limit = limit;
        let report = analyze(&candidate, Some(&query)).unwrap();
        assert!(!report.truncated);
        assert!(!report.incomplete);
        for choice in selected_and_catalog(&report) {
            assert_eq!(choice.availability, Available);
            assert_eq!(choice.compatibility, Incompatible);
        }
    }
    query.search = "args.p".into();
    for limit in [1, 128] {
        query.limit = limit;
        let report = analyze(&candidate, Some(&query)).unwrap();
        assert!(report.truncated && report.incomplete);
        assert_eq!(
            report.selected.as_ref().unwrap().compatibility,
            Incompatible
        );
        if limit == 128 {
            assert!(
                report
                    .diagnostic
                    .as_ref()
                    .unwrap()
                    .contains("budget exhausted")
            );
            assert!(
                report
                    .references
                    .iter()
                    .any(|choice| choice.compatibility == Unknown)
            );
        }
    }
}

#[test]
fn schema_materialization_budget_precedes_large_nested_sibling_copies() {
    let (mut candidate, mut query) = array_reference_candidate(
        "tool",
        &vec![json!({"type":"object","fields":{"wrapped":reference(&["args"])}}); 20],
        &json!({"type":"object"}),
    );
    candidate.block.input = object(
        json!({"text":{"type":"string","description":"x".repeat(4096)}}),
        &["text"],
    );
    let WorkflowNodeDefinition::Tool { arguments, .. } = &candidate.block.nodes["receive"] else {
        panic!("Tool")
    };
    let WorkflowValue::Object { fields } = arguments else {
        panic!("object")
    };
    let WorkflowValue::Array { items } = &fields["values"] else {
        panic!("array")
    };
    let declarations = SchemaMap::from_schema(&candidate.block.input);
    let mut inference = SchemaInference::new(
        &declarations,
        "receive",
        SchemaBudget::limited(16 * 1024, 1024 * 1024, 4096),
    );
    assert!(matches!(
        inference.siblings(items, 19),
        Err(InferenceError::Budget)
    ));
    assert!(inference.budget.allocation_used <= 16 * 1024);
    query.consumer.path[1] = WorkflowExpressionStep::Item { index: 19 };
    query.consumer.path.push(WorkflowExpressionStep::Field {
        key: "wrapped".into(),
    });
    query.reference = Some(vec!["args".into(), "text".into()]);
    query.search = "args.text".into();
    for limit in [1, 128] {
        query.limit = limit;
        let report = analyze_with_schema_budget(
            &candidate,
            Some(&query),
            SchemaBudget::limited(16 * 1024, 1024 * 1024, 4096),
        )
        .unwrap();
        assert!(report.truncated && report.incomplete);
        assert!(
            report
                .diagnostic
                .as_ref()
                .unwrap()
                .contains("budget exhausted")
        );
        for choice in selected_and_catalog(&report) {
            assert_eq!(choice.availability, Available);
            assert_eq!(choice.compatibility, Unknown);
        }
    }
}

#[test]
fn distinct_large_schema_work_and_nested_array_expansion_share_one_request_budget() {
    let items: Vec<_> = (0..24)
        .map(|index| reference(&["args", &format!("p{index:03}")]))
        .collect();
    let (mut candidate, mut query) =
        array_reference_candidate("tool", &items, &json!({"type":"object"}));
    let properties: serde_json::Map<_, _> = (0..24)
        .map(|index| {
            (
                format!("p{index:03}"),
                json!({"type":"object","description":"x".repeat(4096)}),
            )
        })
        .collect();
    let required: Vec<_> = properties.keys().map(String::as_str).collect();
    candidate.block.input = object(json!(properties), &required);
    query.reference = Some(vec!["args".into(), "p000".into()]);
    query.consumer.path[1] = WorkflowExpressionStep::Item { index: 23 };
    query.search = "args.p000".into();
    for limits in [
        (1024 * 1024, 32 * 1024, 4096),
        (1024 * 1024, 1024 * 1024, 2),
    ] {
        let report = analyze_with_schema_budget(
            &candidate,
            Some(&query),
            SchemaBudget::limited(limits.0, limits.1, limits.2),
        )
        .unwrap();
        assert!(report.truncated && report.incomplete);
        for choice in selected_and_catalog(&report) {
            assert_eq!(
                choice.availability,
                if limits.2 == 2 { Pending } else { Available }
            );
            assert_eq!(choice.compatibility, Unknown);
        }
    }
    // Existing multi-layer replacement semantics still hold without a budget
    // failure; a tiny budget cannot turn their unfinished proof into compatible.
    let a = reference(&["args", "a"]);
    let nested = json!({"type":"array","items":[{"type":"object","fields":{"slot":a}}, {"type":"object","fields":{"slot":a}}]});
    let (candidate, mut query) = array_reference_candidate(
        "return",
        &[nested.clone(), nested],
        &json!({"type":"array","items":{"type":"object"}}),
    );
    query.consumer.path.extend([
        WorkflowExpressionStep::Item { index: 1 },
        WorkflowExpressionStep::Field { key: "slot".into() },
    ]);
    query.reference = Some(vec!["args".into(), "b".into()]);
    assert_eq!(selected(&candidate, &query).compatibility, Incompatible);
    let report = analyze_with_schema_budget(
        &candidate,
        Some(&query),
        SchemaBudget::limited(4096, 4096, 4),
    )
    .unwrap();
    assert!(report.truncated && report.incomplete);
    assert_eq!(report.selected.unwrap().compatibility, Unknown);
}

#[test]
fn recursive_schema_proofs_and_reference_paths_share_the_inference_budget() {
    let mut schema = json!({"type":"string"});
    for _ in 0..24 {
        schema = object(json!({"child":schema}), &["child"]);
    }
    let candidate = definition(
        json!({"input":schema,"output":schema,"nodes": {"workflow_start":{"type":"start"},"done":done(reference(&["args"]))},"edges": [{"from":"workflow_start","to":"done"}],}),
    );
    let query = query("done", WorkflowConsumerField::ReturnOutput, &["args"]);
    assert_eq!(selected(&candidate, &query).compatibility, Compatible);
    let report = analyze_with_schema_budget(
        &candidate,
        Some(&query),
        SchemaBudget::limited(1024 * 1024, 1024 * 1024, 100),
    )
    .unwrap();
    assert!(report.truncated && report.incomplete);
    assert_eq!(report.selected.unwrap().compatibility, Unknown);

    // Charge the required-member scan before walking it, and reuse the exact
    // resolved path thereafter without rebuilding an owned required-field set.
    let declarations = SchemaMap::from_schema(&object(
        Value::Object(
            (0..256)
                .map(|i| (format!("p{i:03}"), json!({"type":"string"})))
                .collect(),
        ),
        &(0..256)
            .map(|i| format!("p{i:03}"))
            .collect::<Vec<_>>()
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
    ));
    let path = vec!["args".into(), "p255".into()];
    let mut inference = SchemaInference::new(
        &declarations,
        "done",
        SchemaBudget::limited(4096, 4096, 4096),
    );
    assert!(matches!(
        inference.resolve(&path),
        Err(InferenceError::Budget)
    ));
    assert_eq!(inference.budget.allocation_used, 1024);
    let mut inference = SchemaInference::new(&declarations, "done", SchemaBudget::default());
    let first = inference.resolve(&path).unwrap();
    for _ in 0..1500 {
        assert!(std::ptr::eq(first, inference.resolve(&path).unwrap()));
    }
    assert!(inference.budget.allocation_used <= 1024);
    assert!(!inference.budget.exhausted);
    let missing = vec!["args".into(), "not_declared".into()];
    assert!(matches!(
        inference.resolve(&missing),
        Err(InferenceError::Invalid(_))
    ));
    inference.budget = SchemaBudget::limited(0, 0, 0);
    assert!(std::ptr::eq(first, inference.resolve(&path).unwrap()));
    assert!(matches!(
        inference.resolve(&missing),
        Err(InferenceError::Unavailable)
    ));
}

#[test]
fn finite_schema_proofs_reserve_validator_storage_and_each_value_validation() {
    let values: Vec<_> = (0..64).map(|index| format!("value_{index:03}")).collect();
    let schema = json!({"type":"string","enum":values});
    let declarations = SchemaMap::default();
    let mut inference = SchemaInference::new(
        &declarations,
        "done",
        SchemaBudget::limited(1024, 1024 * 1024, 4096),
    );
    assert!(matches!(
        inference.compatible(&schema, &schema),
        Err(InferenceError::Budget)
    ));
    assert_eq!(inference.budget.allocation_used, 0);
    let mut inference = SchemaInference::new(
        &declarations,
        "done",
        SchemaBudget::limited(1024 * 1024, 64 * 1024, 4096),
    );
    assert!(matches!(
        inference.compatible(&schema, &schema),
        Err(InferenceError::Budget)
    ));
    assert!(inference.budget.allocation_used < 1024 * 1024);
    assert!(super::super::schemas_compatible(&schema, &schema));
    let expected = json!({"type":"array","items":schema});
    let actual = json!({"type":"array","const":vec!["value_000"; 256]});
    let mut inference = SchemaInference::new(
        &declarations,
        "done",
        SchemaBudget::limited(1024 * 1024, 1024 * 1024, 4096),
    );
    assert!(matches!(
        inference.compatible(&actual, &expected),
        Err(InferenceError::Budget)
    ));
    assert!(
        inference.budget.allocation_used > 0,
        "validator compilation was admitted before nested value work was refused"
    );
}

#[test]
fn exhausted_analysis_retains_cached_incompatibility_and_native_scope_facts() {
    let candidate = definition(
        json!({"input":object(json!({"x":{"type":"integer"}}), &["x"]),"output":object(json!({"x":{"type":"string"}}), &["x"]),"nodes": {"workflow_start":{"type":"start"},"done":done(json!({"type":"object","fields":{"x":reference(&["args","x"])}}))},"edges": [{"from":"workflow_start","to":"done"}],}),
    );
    let mut query = query("done", WorkflowConsumerField::ReturnOutput, &["args", "x"]);
    query.consumer.path = vec![WorkflowExpressionStep::Field { key: "x".into() }];
    query.search = "args.x".into();
    let report = analyze_with_schema_budget(
        &candidate,
        Some(&query),
        SchemaBudget::limited(4096, 2000, 4096),
    )
    .unwrap();
    assert!(report.truncated && report.incomplete);
    for choice in selected_and_catalog(&report) {
        assert_eq!(choice.availability, Available);
        assert_eq!(choice.compatibility, Incompatible);
    }
    let (candidate, mut query) = array_reference_candidate("tool", &[], &json!({"type":"object"}));
    query.consumer.path.clear();
    query.reference = Some(vec!["receive".into()]);
    query.search = "receive".into();
    let report =
        analyze_with_schema_budget(&candidate, Some(&query), SchemaBudget::limited(0, 0, 0))
            .unwrap();
    assert_eq!(report.selected.unwrap().availability, Unavailable);
}

#[test]
fn equality_reads_a_known_type_without_materializing_or_measuring_the_schema() {
    let mut keys: Vec<_> = (0..12000).map(|index| format!("p{index:05}")).collect();
    keys.insert(0, "zzflag".into());
    let source = object(
        Value::Object(
            keys.iter()
                .map(|key| (key.clone(), json!({"type":"boolean"})))
                .collect(),
        ),
        &keys.iter().map(String::as_str).collect::<Vec<_>>(),
    );
    let declarations = SchemaMap::from_schema(&source);
    let mut inference = SchemaInference::new(&declarations, "gate", SchemaBudget::default());
    assert!(
        matches!(
            inference.borrowed(&declarations.0["args"]),
            Err(InferenceError::Budget)
        ),
        "the complete schema exceeds the measurement step budget"
    );
    let candidate = definition(
        json!({"input":source,"output":object(json!({}), &[]),"nodes": {"workflow_start":{"type":"start"},"gate":{"type":"branch","cases": [{"id":"true", "condition":{"type":"equal","left":reference(&["args","p11999"]),"right":reference(&["args"])}}]},"done":done(json!({"type":"literal","value":{}}))},"edges": [{"from":"workflow_start","to":"gate"},{"from":"gate","to":"done","port":"true"},{"from":"gate","to":"done","port":"false"}],}),
    );
    let mut query = query(
        "gate",
        WorkflowConsumerField::Condition {
            case_id: "true".into(),
        },
        &["args", "p11999"],
    );
    query.consumer.path = vec![WorkflowExpressionStep::Left];
    query.search = "args.p11999".into();
    let report = analyze(&candidate, Some(&query)).unwrap();
    for choice in selected_and_catalog(&report) {
        assert_eq!(choice.availability, Available);
        assert_eq!(choice.compatibility, Incompatible);
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
        _ => {
            let carry = receiver == "carry";
            let body_output = if carry { source.clone() } else { empty.clone() };
            let body_value = if carry {
                json!({"x":"known"})
            } else {
                json!({})
            };
            (
                json!({"type":"loop","input":if carry { json!({"type":"literal","value":{}}) } else { value.clone() },"body":{"input":destination,"output":body_output,"nodes": {"workflow_start":{"type":"start"},"done":done(json!({"type":"literal","value":body_value}))},"edges": [{"from":"workflow_start","to":"done"}],},"until":{"type":"boolean","value":{"type":"literal","value":true}},"carry":if carry { value } else { json!({"type":"literal","value":{}}) },"max_iterations":1}),
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
            json!({"input":source,"output":output,"nodes": {"workflow_start":{"type":"start"},"receive":node},"edges": [{"from":"workflow_start","to":"receive"}],}),
        )
    } else {
        let edges = if receiver == "parallel" {
            json!([{"from":"receive","to":"done"}])
        } else {
            json!([{"from":"receive","to":"done","port":"satisfied"},{"from":"receive","to":"done","port":"exhausted"}])
        };
        definition(
            json!({"input":source,"output":output,"nodes": {"workflow_start":{"type":"start"},"receive":node,"done":done(json!({"type":"literal","value":{}}))},"edges": ({ let mut edges = serde_json::to_value(edges).unwrap(); edges.as_array_mut().unwrap().insert(0, serde_json::json!({"from":"workflow_start","to":"receive"})); edges }),}),
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

fn array_reference_candidate(
    receiver: &str,
    items: &[Value],
    element: &Value,
) -> (WorkflowDefinition, WorkflowAnalysisQuery) {
    let left = object(json!({"left":{"type":"string"}}), &["left"]);
    let right = object(json!({"right":{"type":"string"}}), &["right"]);
    let source = object(
        json!({"a":left,"b":right,"open":{"type":"object"},"text":{"type":"string"}}),
        &["a", "b", "open", "text"],
    );
    let destination = object(json!({"values":{"type":"array","items":element}}), &[]);
    let empty = object(json!({}), &[]);
    let value = json!({"type":"object","fields":{"values":{"type":"array","items":items}}});
    let literal = json!({"type":"literal","value":{}});
    let (node, field, output) = match receiver {
        "return" => (
            done(value),
            WorkflowConsumerField::ReturnOutput,
            destination.clone(),
        ),
        "tool" => (
            json!({"type":"tool","selector":{"origin":"builtin","name":"read"},"arguments":value,"result":{"type":"text","part":0}}),
            WorkflowConsumerField::Arguments,
            empty.clone(),
        ),
        _ => {
            let carry = receiver == "carry";
            let body_output = if carry { source.clone() } else { empty.clone() };
            let body_value = if carry {
                json!({"type":"literal","value":{"a":{"left":"a"},"b":{"right":"b"},"open":{},"text":"text"}})
            } else {
                literal.clone()
            };
            (
                json!({"type":"loop","input":if carry { literal.clone() } else { value.clone() },"body":{"input":destination,"output":body_output,"nodes": {"workflow_start":{"type":"start"},"done":done(body_value)},"edges": [{"from":"workflow_start","to":"done"}],},"until":{"type":"boolean","value":{"type":"literal","value":true}},"carry":if carry { value } else { literal.clone() },"max_iterations":1}),
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
            json!({"input":source,"output":output,"nodes": {"workflow_start":{"type":"start"},"receive":node},"edges": [{"from":"workflow_start","to":"receive"}],}),
        )
    } else {
        let edges = if matches!(receiver, "input" | "carry") {
            json!([{"from":"receive","to":"done","port":"satisfied"},{"from":"receive","to":"done","port":"exhausted"}])
        } else {
            json!([{"from":"receive","to":"done"}])
        };
        definition(
            json!({"input":source,"output":output,"nodes": {"workflow_start":{"type":"start"},"receive":node,"done":done(literal)},"edges": ({ let mut edges = serde_json::to_value(edges).unwrap(); edges.as_array_mut().unwrap().insert(0, serde_json::json!({"from":"workflow_start","to":"receive"})); edges }),}),
        )
    };
    let mut query = query(
        "receive",
        field,
        &[
            if receiver == "carry" {
                "result"
            } else {
                "args"
            },
            "a",
        ],
    );
    query.consumer.path = vec![
        WorkflowExpressionStep::Field {
            key: "values".into(),
        },
        WorkflowExpressionStep::Item { index: 1 },
    ];
    (candidate, query)
}

fn replace_array_reference(candidate: &mut WorkflowDefinition, query: &WorkflowAnalysisQuery) {
    let node = candidate.block.nodes.get_mut("receive").unwrap();
    let mut value = match node {
        WorkflowNodeDefinition::Return { output } => output,
        WorkflowNodeDefinition::Tool { arguments, .. } => arguments,
        WorkflowNodeDefinition::Loop { carry, .. }
            if matches!(query.consumer.field, WorkflowConsumerField::LoopCarry) =>
        {
            carry
        }
        WorkflowNodeDefinition::Loop { input, .. } => input,
        _ => panic!("array receiver"),
    };
    for step in &query.consumer.path {
        value = match (value, step) {
            (WorkflowValue::Object { fields }, WorkflowExpressionStep::Field { key }) => {
                fields.get_mut(key).unwrap()
            }
            (WorkflowValue::Array { items }, WorkflowExpressionStep::Item { index }) => {
                items.get_mut(*index as usize).unwrap()
            }
            _ => panic!("value path"),
        };
    }
    *value = WorkflowValue::Reference {
        path: query.reference.clone().unwrap(),
    };
}
