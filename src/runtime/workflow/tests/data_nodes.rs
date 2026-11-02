use super::*;
use crate::runtime::workflow::transform;

fn document(value: Value) -> WorkflowDefinition {
    serde_json::from_value(value).unwrap()
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
    assert!(transform::validate_template("{{ missing }}", &variables).is_err());
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
fn list_operations_are_ordered_stable_and_keep_exact_integer_order() {
    let filter =
        serde_json::from_value(json!({"path":["score"],"operator":"greater_equal","value":2}))
            .unwrap();
    let sort = serde_json::from_value(json!({"path":["score"],"order":"desc"})).unwrap();
    let output = transform::list(json!([{"name":"a","score":1},{"name":"b","score":3},{"name":"c","score":3},{"name":"d","score":2}]),Some(&filter),Some(&sort),Some(2)).unwrap();
    assert_eq!(
        output["result"],
        json!([{"name":"b","score":3},{"name":"c","score":3}])
    );
    assert_eq!(output["first_record"]["name"], "b");
    assert_eq!(output["last_record"]["name"], "c");
    let sort = serde_json::from_value(json!({"path":[],"order":"asc"})).unwrap();
    assert_eq!(
        transform::list(
            json!([9_007_199_254_740_993_u64, 9_007_199_254_740_992_u64]),
            None,
            Some(&sort),
            None
        )
        .unwrap()["result"],
        json!([9_007_199_254_740_992_u64, 9_007_199_254_740_993_u64])
    );
    assert_eq!(
        transform::list(json!([]), None, None, None).unwrap(),
        json!({"result":[]})
    );
    assert_eq!(
        transform::list(
            json!([9_007_199_254_740_993_u64, 9_007_199_254_740_992.0]),
            None,
            Some(&sort),
            None
        )
        .unwrap()["result"],
        json!([9_007_199_254_740_992.0, 9_007_199_254_740_993_u64])
    );
    assert!(transform::list(json!(["x".repeat(MAX_VALUE_BYTES / 2)]), None, None, None).is_err());
}

fn aggregate_document() -> WorkflowDefinition {
    document(json!({"description":"Converge exclusive branches","block":{
        "input":{"type":"object","properties":{"flag":{"type":"boolean"}},"required":["flag"],"additionalProperties":false},
        "nodes":{
            "start":{"type":"start"},
            "choose":{"type":"branch","cases":[{"id":"true","condition":{"type":"boolean","value":{"type":"reference","path":["args","flag"]}}}]},
            "left":{"type":"template","variables":{},"template":"Left"},
            "right":{"type":"template","variables":{},"template":"Right"},
            "merge":{"type":"aggregate","groups":[{"name":"output","output_type":"string","variables":[["left","output"],["right","output"]]}]},
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
fn aggregator_rejects_incomplete_coverage_concurrent_sources_and_wrong_types() {
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
    assert!(
        compile_test(parallel)
            .unwrap_err()
            .to_string()
            .contains("mutually exclusive")
    );
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
fn classifier_has_finite_model_demand_and_class_specific_exits() {
    let value = json!({"description":"Classify once","block":{"input":{"type":"object","properties":{"query":{"type":"string"}},"required":["query"],"additionalProperties":false},"nodes":{"start":{"type":"start"},"classify":{"type":"classifier","input":{"type":"reference","path":["args","query"]},"classes":[{"id":"support","description":"Support requests"},{"id":"other","description":"Other questions"}]},"done":{"type":"return","outputs":[{"variable":"category","value_selector":["classify","structured_output","class_id"]}]}},"edges":[{"from":"start","to":"classify"},{"from":"classify","to":"done","port":"support"},{"from":"classify","to":"done","port":"other"}]}});
    let program = compile_test(document(value.clone())).unwrap();
    assert_eq!(program.model_call_bound, 1);
    assert_eq!(
        program.inspect().blocks["block"].nodes["classify"].kind,
        "classifier"
    );
    let mut missing = value.clone();
    missing["block"]["edges"].as_array_mut().unwrap().pop();
    assert!(compile_test(document(missing)).is_err());
    let mut duplicate = value;
    duplicate["block"]["nodes"]["classify"]["classes"][1]["id"] = json!("support");
    assert!(compile_test(document(duplicate)).is_err());
}

#[cfg(unix)]
#[tokio::test]
async fn filtered_list_schema_and_values_reach_a_template_through_the_native_runtime() {
    let definition = document(json!({"description":"Format top scores","block":{
        "input":{"type":"object","properties":{"scores":{"type":"array","items":{"type":"integer"}}},"required":["scores"],"additionalProperties":false},
        "nodes":{"start":{"type":"start"},"select":{"type":"list","input":{"type":"reference","path":["args","scores"]},"filter":{"path":[],"operator":"greater","value":1},"sort":{"path":[],"order":"desc"},"limit":2},"format":{"type":"template","variables":{"scores":{"type":"reference","path":["select","result"]}},"template":"{{ scores | join(', ') }}"},"done":{"type":"return","outputs":[{"variable":"scores","value_selector":["format","output"]}]}},
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
