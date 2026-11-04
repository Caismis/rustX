use super::*;
use crate::runtime::workflow::transform;

fn document(value: Value) -> WorkflowDefinition {
    serde_json::from_value(value).unwrap()
}

fn run_list(
    input: Value,
    filter: Option<&super::super::WorkflowListFilter>,
    sort: Option<&super::super::WorkflowListSort>,
    limit: Option<u32>,
) -> Result<Value, WorkflowRunError> {
    transform::list(input, filter, sort, limit, None, |value| match value {
        WorkflowValue::Literal { value } => Ok(value.clone()),
        _ => panic!("literal expected in this fixture"),
    })
}

#[test]
fn template_compilation_checks_bindings_and_rendering_is_bounded() {
    let variables = BTreeMap::from([("items".into(), reference("args.items"))]);
    let template = "{% for item in items %}{% if item.score > 1 %}{{ item.name | upper }};{% endif %}{% endfor %}";
    transform::validate_template(template, &variables).unwrap();
    let value = transform::render(template, &variables, |_| {
        Ok(json!({"items":[{"name":"a","score":1},{"name":"b","score":2}]}))
    })
    .unwrap();
    assert_eq!(value, json!({"output":"B;"}));
    assert!(transform::validate_template("{{ missing }}", &variables).is_ok());
    assert!(transform::validate_template("{% if %}", &variables).is_err());
    assert!(transform::render("{{ items.missing }}", &variables, |_| Ok(json!({}))).is_err());
    assert!(
        transform::render(&"x".repeat(MAX_VALUE_BYTES + 1), &BTreeMap::new(), |_| Ok(
            json!({})
        ))
        .is_err()
    );
}

#[test]
fn template_matches_dify_undefined_whitespace_and_common_jinja_operations() {
    // Expectations follow Dify's CodeExecutor Jinja2 SandboxedEnvironment,
    // rather than Graphon's separate default/fallback renderer.
    let variables = BTreeMap::from([
        ("text".into(), reference("args.text")),
        ("items".into(), reference("args.items")),
        ("data".into(), reference("args.data")),
    ]);
    let context = json!({"text":"  <hello>  ","items":[1,2,3],"data":{"name":"world"}});
    for (source, expected) in [
        (
            "{{ missing }}|{{ missing | default('fallback') }}",
            "|fallback",
        ),
        ("{% if missing %}yes{% else %}no{% endif %}", "no"),
        (
            "{{ data.missing }}|{{ data.missing | default('fallback') }}",
            "|fallback",
        ),
        ("{{ text.strip().upper() }}", "<HELLO>"),
        (
            "{% for key, value in data.items() %}{{ key }}={{ value }}{% endfor %}",
            "name=world",
        ),
        ("{{ items | sum }}|{{ items | join(',') }}", "6|1,2,3"),
        ("{{ range(3) | join(',') }}", "0,1,2"),
        ("a\n{{- text | trim -}}\nb\n", "a<hello>b"),
        ("{{ true }}|{{ false }}|{{ none }}", "True|False|None"),
    ] {
        transform::validate_template(source, &variables).unwrap();
        let result = transform::render(source, &variables, |_| Ok(context.clone())).unwrap();
        assert_eq!(result["output"], expected, "{source}");
    }
    let json_output =
        transform::render("{{ items | tojson }}", &variables, |_| Ok(context.clone())).unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(json_output["output"].as_str().unwrap()).unwrap(),
        json!([1, 2, 3])
    );
    assert!(transform::render("{{ missing.value }}", &variables, |_| Ok(context.clone())).is_err());
    assert!(
        transform::render(
            "{{ text.__class__() }}",
            &variables,
            |_| Ok(context.clone())
        )
        .is_err()
    );
    assert!(
        transform::render("{% include 'file' %}", &variables, |_| Ok(context.clone())).is_err()
    );
    assert!(
        transform::render("{{ range(100001) | length }}", &variables, |_| Ok(
            context.clone()
        ))
        .is_err()
    );
    assert!(
        transform::render(
            "{% for a in range(10000) %}{% for b in range(10000) %}{% endfor %}{% endfor %}",
            &variables,
            |_| Ok(context.clone())
        )
        .is_err()
    );
}

#[test]
fn list_operations_are_ordered_stable_and_keep_exact_integer_order() {
    let filter = serde_json::from_value(
        json!({"path":["score"],"operator":"greater_equal","value":{"type":"literal","value":2}}),
    )
    .unwrap();
    let sort = serde_json::from_value(json!({"path":["score"],"order":"desc"})).unwrap();
    let output = run_list(json!([{"name":"a","score":1},{"name":"b","score":3},{"name":"c","score":3},{"name":"d","score":2}]),Some(&filter),Some(&sort),Some(2)).unwrap();
    assert_eq!(
        output["result"],
        json!([{"name":"b","score":3},{"name":"c","score":3}])
    );
    assert_eq!(output["first_record"]["name"], "b");
    assert_eq!(output["last_record"]["name"], "c");
    let sort = serde_json::from_value(json!({"path":[],"order":"asc"})).unwrap();
    assert_eq!(
        run_list(
            json!([9_007_199_254_740_993_u64, 9_007_199_254_740_992_u64]),
            None,
            Some(&sort),
            None
        )
        .unwrap()["result"],
        json!([9_007_199_254_740_992_u64, 9_007_199_254_740_993_u64])
    );
    assert_eq!(
        run_list(json!([]), None, None, None).unwrap(),
        json!({"result":[],"first_record":null,"last_record":null})
    );
    assert_eq!(
        run_list(
            json!([9_007_199_254_740_993_u64, 9_007_199_254_740_992.0]),
            None,
            Some(&sort),
            None
        )
        .unwrap()["result"],
        json!([9_007_199_254_740_992.0, 9_007_199_254_740_993_u64])
    );
    assert!(run_list(json!(["x".repeat(MAX_VALUE_BYTES / 2)]), None, None, None).is_err());
}

fn aggregate_document() -> WorkflowDefinition {
    document(json!({"description":"Converge exclusive branches","block":{
        "input":{"type":"object","properties":{"flag":{"type":"boolean"}},"required":["flag"],"additionalProperties":false},
        "nodes":{
            "start":{"type":"start"},
            "choose":{"type":"branch","cases":[{"id":"true","condition":{"type":"boolean","value":{"type":"reference","path":["args","flag"]}}}]},
            "left":{"type":"template","variables":{},"template":"Left"},
            "right":{"type":"template","variables":{},"template":"Right"},
            "merge":{"type":"aggregate","grouped":false,"groups":[{"name":"output","output_type":"string","variables":[["left","output"],["right","output"]]}]},
            "format":{"type":"template","variables":{"answer":{"type":"reference","path":["merge","output"]}},"template":"{{ answer | upper }}!"},
            "done":{"type":"return","outputs":[{"variable":"answer","value_selector":["format","output"]}]}
        },"edges":[{"from":"start","to":"choose"},{"from":"choose","to":"left","port":"true"},{"from":"choose","to":"right","port":"false"},{"from":"left","to":"merge"},{"from":"right","to":"merge"},{"from":"merge","to":"format"},{"from":"format","to":"done"}]
    }}))
}

#[cfg(unix)]
#[tokio::test]
async fn both_exclusive_paths_converge_and_feed_one_native_template() {
    for (flag, expected) in [(true, "LEFT!"), (false, "RIGHT!")] {
        let plane = workflow_test_plane(1);
        let runtime = workflow_runtime(&plane);
        let (_, cancellation) = workflow_cancellation();
        let result = runtime
            .run_test_foreground(
                Arc::new(compile_test(aggregate_document()).unwrap()),
                ToolCallId::new("data-nodes"),
                workflow_test_context(&plane),
                json!({"flag":flag}),
                cancellation,
            )
            .await
            .unwrap();
        assert_eq!(result, json!({"answer":expected}));
    }
}

#[test]
fn aggregator_validates_coverage_and_types_but_accepts_ordered_parallel_sources() {
    let original = aggregate_document();
    let mut missing = serde_json::to_value(&original).unwrap();
    missing["block"]["nodes"]["merge"]["groups"][0]["variables"] = json!([["left", "output"]]);
    assert!(
        compile_test(document(missing))
            .unwrap_err()
            .to_string()
            .contains("cover every activation")
    );
    let mut parallel = original.clone();
    parallel.block.nodes.remove("choose");
    parallel
        .block
        .edges
        .retain(|e| e.from != "choose" && e.to != "choose");
    parallel
        .block
        .edges
        .extend([edge("start", "left"), edge("start", "right")]);
    assert!(compile_test(parallel).is_ok());
    let mut wrong = serde_json::to_value(&original).unwrap();
    wrong["block"]["nodes"]["merge"]["groups"][0]["output_type"] = json!("number");
    assert!(compile_test(document(wrong)).is_err());
    let mut ordinary = original;
    ordinary.block.nodes.insert(
        "format".into(),
        WorkflowNodeDefinition::Template {
            variables: BTreeMap::from([("answer".into(), reference("left.output"))]),
            template: "{{ answer }}".into(),
        },
    );
    assert!(matches!(
        compile_test(ordinary).unwrap_err().cause(),
        WorkflowCompileError::InvalidReference(_)
    ));
}

#[test]
fn authoring_offers_exclusive_sources_only_to_aggregator_consumers() {
    use crate::runtime::workflow::authoring::*;
    let definition = aggregate_document();
    let query:WorkflowAnalysisQuery=serde_json::from_value(json!({"consumer":{"address":{"block":[],"node":"merge"},"field":{"kind":"aggregate_variable","group":0,"index":0}},"reference":["left","output"],"limit":32})).unwrap();
    let analysis = analyze(&definition, Some(&query), &BTreeSet::new()).unwrap();
    assert_eq!(
        analysis.selected.unwrap().availability,
        WorkflowReferenceAvailability::Available
    );
    let query:WorkflowAnalysisQuery=serde_json::from_value(json!({"consumer":{"address":{"block":[],"node":"format"},"field":{"kind":"template_variable","name":"answer"}},"reference":["left","output"],"limit":32})).unwrap();
    let analysis = analyze(&definition, Some(&query), &BTreeSet::new()).unwrap();
    assert_eq!(
        analysis.selected.unwrap().availability,
        WorkflowReferenceAvailability::Unavailable
    );
}

#[test]
fn template_insertion_queries_the_native_entry_scope_before_binding_exists() {
    use crate::runtime::workflow::authoring::*;
    let definition = aggregate_document();
    for (path, availability) in [
        (
            vec!["merge", "output"],
            WorkflowReferenceAvailability::Available,
        ),
        (
            vec!["left", "output"],
            WorkflowReferenceAvailability::Unavailable,
        ),
        (
            vec!["format", "output"],
            WorkflowReferenceAvailability::Unavailable,
        ),
    ] {
        let query:WorkflowAnalysisQuery=serde_json::from_value(json!({"consumer":{"address":{"block":[],"node":"format"},"field":{"kind":"template_variable","name":"new_input"}},"reference":path,"limit":32})).unwrap();
        let analysis = analyze(&definition, Some(&query), &BTreeSet::new()).unwrap();
        assert_eq!(analysis.selected.unwrap().availability, availability);
    }
}

#[test]
fn classifier_has_finite_model_demand_and_class_specific_exits() {
    let value = json!({"description":"Classify once","block":{"input":{"type":"object","properties":{"query":{"type":"string"}},"required":["query"],"additionalProperties":false},"nodes":{"start":{"type":"start"},"classify":{"type":"classifier","input":{"type":"reference","path":["args","query"]},"classes":[{"id":"support","label":"Support","description":[{"type":"literal","value":"Support requests"}]},{"id":"other","label":"Other","description":[{"type":"literal","value":"Other questions"}]}]},"done":{"type":"return","outputs":[{"variable":"category","value_selector":["classify","class_id"]}]}},"edges":[{"from":"start","to":"classify"},{"from":"classify","to":"done","port":"support"},{"from":"classify","to":"done","port":"other"}]}});
    let program = compile_test(document(value.clone())).unwrap();
    assert_eq!(program.model_call_bound, 1);
    assert_eq!(
        program.inspect().blocks["block"].nodes["classify"].kind,
        "classifier"
    );
    let mut missing = value.clone();
    missing["block"]["edges"].as_array_mut().unwrap().pop();
    assert!(compile_test(document(missing)).is_err());
    for kind in ["number", "integer"] {
        let mut numeric = value.clone();
        numeric["block"]["input"]["properties"]["query"] = json!({"type":kind});
        assert!(
            compile_test(document(numeric)).is_ok(),
            "numeric variable input"
        );
    }
    let mut empty = value.clone();
    empty["block"]["nodes"]["classify"]["classes"][0]["description"] =
        json!([{ "type":"literal", "value":"   " }]);
    assert!(compile_test(document(empty)).is_err());
    let mut duplicate = value;
    duplicate["block"]["nodes"]["classify"]["classes"][1]["id"] = json!("support");
    assert!(compile_test(document(duplicate)).is_err());
}

#[cfg(unix)]
#[tokio::test]
async fn filtered_list_schema_and_values_reach_a_template_through_the_native_runtime() {
    let definition = document(json!({"description":"Format top scores","block":{
        "input":{"type":"object","properties":{"scores":{"type":"array","items":{"type":"integer"}}},"required":["scores"],"additionalProperties":false},
        "nodes":{"start":{"type":"start"},"select":{"type":"list","input":{"type":"reference","path":["args","scores"]},"filter":{"path":[],"operator":"greater","value":{"type":"literal","value":1}},"sort":{"path":[],"order":"desc"},"limit":2},"format":{"type":"template","variables":{"scores":{"type":"reference","path":["select","result"]}},"template":"{{ scores | join(', ') }}"},"done":{"type":"return","outputs":[{"variable":"scores","value_selector":["format","output"]}]}},
        "edges":[{"from":"start","to":"select"},{"from":"select","to":"format"},{"from":"format","to":"done"}]}}));
    let plane = workflow_test_plane(1);
    let runtime = workflow_runtime(&plane);
    let (_, cancellation) = workflow_cancellation();
    let result = runtime
        .run_test_foreground(
            Arc::new(compile_test(definition).unwrap()),
            ToolCallId::new("list-template"),
            workflow_test_context(&plane),
            json!({"scores":[1,2,4,3]}),
            cancellation,
        )
        .await
        .unwrap();
    assert_eq!(result, json!({"scores":"4, 3"}));
}

#[cfg(unix)]
#[tokio::test]
async fn aggregator_selects_first_available_source_and_emits_real_group_output_paths() {
    for grouped in [false, true] {
        for reverse in [false, true] {
            let mut definition = aggregate_document();
            definition.block.nodes.remove("choose");
            definition
                .block
                .edges
                .retain(|edge| edge.from != "choose" && edge.to != "choose");
            definition
                .block
                .edges
                .extend([edge("start", "left"), edge("start", "right")]);
            if let WorkflowNodeDefinition::Aggregate {
                groups,
                grouped: setting,
            } = definition.block.nodes.get_mut("merge").unwrap()
            {
                *setting = grouped;
                if grouped {
                    groups[0].name = "Group1".into();
                }
                if reverse {
                    groups[0].variables.reverse();
                }
            }
            if grouped
                && let WorkflowNodeDefinition::Template { variables, .. } =
                    definition.block.nodes.get_mut("format").unwrap()
            {
                variables.insert("answer".into(), reference("merge.Group1.output"));
            }
            let plane = workflow_test_plane(2);
            let runtime = workflow_runtime(&plane);
            let (_, cancellation) = workflow_cancellation();
            let result = runtime
                .run_test_foreground(
                    Arc::new(compile_test(definition).unwrap()),
                    ToolCallId::new("ordered-aggregation"),
                    workflow_test_context(&plane),
                    json!({"flag":true}),
                    cancellation,
                )
                .await
                .unwrap();
            assert_eq!(result, json!({"answer":if reverse {"RIGHT!"}else{"LEFT!"}}));
        }
    }
}

#[test]
fn list_filters_then_extracts_one_based_serial_before_sort_and_limit() {
    let filter:super::super::WorkflowListFilter=serde_json::from_value(json!({"path":[],"operator":"greater","value":{"type":"reference","path":["args","threshold"]}})).unwrap();
    let sort = serde_json::from_value(json!({"path":[],"order":"desc"})).unwrap();
    let extract = reference("args.serial");
    let result = transform::list(
        json!([5, 1, 2, 4]),
        Some(&filter),
        Some(&sort),
        Some(1),
        Some(&extract),
        |value| match value {
            WorkflowValue::Reference { path } => Ok(if path.last().unwrap() == "threshold" {
                json!(1)
            } else {
                json!(2)
            }),
            _ => unreachable!(),
        },
    )
    .unwrap();
    assert_eq!(
        result,
        json!({"result":[2],"first_record":2,"last_record":2})
    );
    for serial in [json!(0), json!(5), json!(1.5), json!("bad")] {
        assert!(
            transform::list(
                json!([1, 2]),
                None,
                None,
                None,
                Some(&WorkflowValue::Literal { value: serial }),
                |v| if let WorkflowValue::Literal { value } = v {
                    Ok(value.clone())
                } else {
                    unreachable!()
                }
            )
            .is_err()
        );
    }
    let result = transform::list(
        json!([]),
        None,
        None,
        None,
        Some(&WorkflowValue::Literal { value: json!(0) }),
        |_| panic!("empty input must not evaluate operations"),
    )
    .unwrap();
    assert_eq!(
        result,
        json!({"result":[],"first_record":null,"last_record":null})
    );
    for (operator, expected) in [
        ("empty", json!([""])),
        ("not_empty", json!(["a", "b"])),
        ("in", json!(["", "a"])),
        ("not_in", json!(["b"])),
    ] {
        let filter = serde_json::from_value(
            json!({"path":[],"operator":operator,"value":{"type":"literal","value":"a"}}),
        )
        .unwrap();
        assert_eq!(
            run_list(json!(["", "a", "b"]), Some(&filter), None, None).unwrap()["result"],
            expected
        );
    }
}

#[test]
fn integer_list_comparisons_admit_numeric_references_without_rounding_the_items() {
    use crate::runtime::workflow::authoring::*;
    let definition = document(
        json!({"description":"typed list","block":{"input":{"type":"object","properties":{"items":{"type":"array","items":{"type":"integer"}},"threshold":{"type":"number"}},"required":["items","threshold"],"additionalProperties":false},"nodes":{"start":{"type":"start"},"select":{"type":"list","input":{"type":"reference","path":["args","items"]},"filter":{"path":[],"operator":"greater","value":{"type":"reference","path":["args","threshold"]}}},"done":{"type":"return","outputs":[{"variable":"result","value_selector":["select","result"]}]}},"edges":[{"from":"start","to":"select"},{"from":"select","to":"done"}]}}),
    );
    compile_test(definition.clone()).unwrap();
    let query=serde_json::from_value(json!({"consumer":{"address":{"block":[],"node":"select"},"field":{"kind":"list_filter_value"}},"reference":["args","threshold"],"limit":32})).unwrap();
    let analysis = analyze(&definition, Some(&query), &BTreeSet::new()).unwrap();
    assert_eq!(
        analysis.selected.unwrap().compatibility,
        WorkflowReferenceCompatibility::Compatible
    );
    let WorkflowNodeDefinition::List { filter, .. } = &definition.block.nodes["select"] else {
        unreachable!()
    };
    let result = transform::list(json!([1, 2]), filter.as_ref(), None, None, None, |_| {
        Ok(json!(1.5))
    })
    .unwrap();
    assert_eq!(result["result"], json!([2]));
}

#[test]
fn disabled_aggregation_retains_but_does_not_read_or_validate_inactive_groups() {
    let mut definition = aggregate_document();
    let WorkflowNodeDefinition::Aggregate { groups, .. } =
        definition.block.nodes.get_mut("merge").unwrap()
    else {
        unreachable!()
    };
    groups[0].name = "Retained".into();
    groups.push(super::super::WorkflowAggregateGroup {
        name: "Inactive".into(),
        output_type: None,
        variables: vec![],
    });
    let program = compile_test(definition).unwrap();
    let view = program.inspect();
    assert!(view.blocks["block"].nodes["merge"].output_schema.is_some());
}
