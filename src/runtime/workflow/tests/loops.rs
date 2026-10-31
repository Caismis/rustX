//! Dify's Loop and Iteration: bodies that read the variables around them,
//! Loop variables written by an Assigner, break conditions, Exit Loop, and
//! Iteration rounds with parallelism and error modes.
use super::*;

/// Records each call and answers `passed` from the call count.
struct Checker {
    starts: AtomicUsize,
    satisfied_at: usize,
    received: std::sync::Mutex<Vec<ToolInvocation>>,
}
impl ToolExecutor for Checker {
    fn progress_capability(&self) -> crate::tools::deadline::ToolProgressCapability {
        crate::tools::deadline::ToolProgressCapability::None
    }
    fn start<'a>(
        &'a self,
        invocation: ToolInvocation,
        context: ToolExecutionContext<'a>,
    ) -> ToolExecutionHandle<'a> {
        ToolExecutionHandle::settled_by_operation(
            Box::pin(async move {
                self.received.lock().unwrap().push(invocation);
                let count = self.starts.fetch_add(1, Ordering::SeqCst) + 1;
                let mut result = terminal(ToolExecutionStatus::Success);
                result.content = vec![ToolResultContent::Json {
                    value: json!({"passed":count >= self.satisfied_at, "label":format!("committed-{count}")}),
                }];
                result
            }),
            context.cancellation,
        )
    }
}

fn checker(satisfied_at: usize) -> Arc<Checker> {
    Arc::new(Checker {
        starts: AtomicUsize::new(0),
        satisfied_at,
        received: std::sync::Mutex::new(Vec::new()),
    })
}

/// Echoes its label after a pause, fails on `bad`, and records how many
/// calls ran at once.
#[derive(Default)]
struct Echo {
    active: AtomicUsize,
    peak: AtomicUsize,
    starts: AtomicUsize,
}
impl ToolExecutor for Echo {
    fn progress_capability(&self) -> crate::tools::deadline::ToolProgressCapability {
        crate::tools::deadline::ToolProgressCapability::None
    }
    fn start<'a>(
        &'a self,
        invocation: ToolInvocation,
        context: ToolExecutionContext<'a>,
    ) -> ToolExecutionHandle<'a> {
        ToolExecutionHandle::settled_by_operation(
            Box::pin(async move {
                self.starts.fetch_add(1, Ordering::SeqCst);
                let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
                self.peak.fetch_max(active, Ordering::SeqCst);
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                self.active.fetch_sub(1, Ordering::SeqCst);
                let label = invocation.arguments["label"].clone();
                if label == "bad" {
                    return terminal(ToolExecutionStatus::Failed {
                        error: "bad item".into(),
                    });
                }
                let mut result = terminal(ToolExecutionStatus::Success);
                result.content = vec![ToolResultContent::Json {
                    value: json!({"passed":true, "label":label}),
                }];
                result
            }),
            context.cancellation,
        )
    }
}

fn state() -> Value {
    schema(
        json!({"passed":{"type":"boolean"},"label":{"type":"string"}}),
        &["passed", "label"],
    )
}

fn check_node(arguments: &Value) -> Value {
    json!({"type":"tool","selector":{"origin":"builtin","name":"check"},"arguments":arguments,"result":{"type":"json","schema":state()}})
}

fn path(path: &[&str]) -> Value {
    json!({"type":"reference","path":path})
}

/// Root `workflow_start -> <id> -> done`, returning the container's value.
fn around(id: &str, input: &Value, output: &Value, container: &Value) -> WorkflowDefinition {
    serde_json::from_value(
        json!({"description":"container","timeout_ms":600_000,"block":{
        "input":input,"output":output,"nodes":{"workflow_start":{"type":"start"},id:container,
            "done":{"type":"return","output":path(&[id])}},
        "edges":[{"from":"workflow_start","to":id},{"from":id,"to":"done"}]}}),
    )
    .unwrap()
}

/// A Loop that checks its variables and saves the check's answer in them,
/// until `passed` holds.
fn checker_loop(loop_count: u32) -> WorkflowDefinition {
    around(
        "feedback",
        &state(),
        &state(),
        &json!({"type":"loop","loop_count":loop_count,
            "loop_variables":[
                {"label":"passed","var_type":"boolean","value":{"type":"literal","value":false}},
                {"label":"label","var_type":"string","value":path(&["args","label"])}],
            "break_condition":{"type":"boolean","value":path(&["feedback","passed"])},
            "body":{"nodes":{"start":{"type":"start"},
                "check":check_node(&json!({"type":"object","fields":{
                    "passed":path(&["feedback","passed"]),"label":path(&["feedback","label"])}})),
                "save":{"type":"assigner","items":[
                    {"variable":["feedback","passed"],"operation":"overwrite","value":path(&["check","passed"])},
                    {"variable":["feedback","label"],"operation":"overwrite","value":path(&["check","label"])}]}},
                "edges":[{"from":"start","to":"check"},{"from":"check","to":"save"}]}}),
    )
}

/// An Iteration over `args.items` whose rounds echo their item.
fn echo_iteration(extra: &Value) -> WorkflowDefinition {
    let mut iteration = json!({"type":"iteration","iterator":path(&["args","items"]),"output":path(&["check","label"]),
        "body":{"nodes":{"start":{"type":"start"},
            "check":check_node(&json!({"type":"object","fields":{
                "passed":{"type":"literal","value":true},"label":path(&["each","item"])}}))},
            "edges":[{"from":"start","to":"check"}]}});
    iteration
        .as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    around(
        "each",
        &schema(
            json!({"items":{"type":"array","items":{"type":"string"}}}),
            &["items"],
        ),
        &schema(json!({"output":{"type":"array"}}), &["output"]),
        &iteration,
    )
}

async fn run(
    plane: &WorkflowTestPlane,
    runtime: WorkflowRuntime,
    executor: Arc<dyn ToolExecutor>,
    workflow: WorkflowDefinition,
    input: Value,
) -> Result<Value, WorkflowRunError> {
    // Parallel rounds may call the Tool at once.
    let mut tool = definition();
    tool.concurrency_policy = ToolConcurrencyPolicy::Parallel;
    let context = context_with_registration(
        plane,
        ToolRegistration::plain(tool, executor),
        crate::agent::AttemptLifecycle::default(),
    );
    let (_, cancellation) = workflow_cancellation();
    runtime
        .run_test_foreground(
            Arc::new(compile_test(workflow).unwrap()),
            ToolCallId::new("container"),
            context,
            input,
            cancellation,
        )
        .await
}

fn count(events: &[RuntimeEvent], matches: impl Fn(&RuntimeEvent) -> bool) -> usize {
    events.iter().filter(|event| matches(event)).count()
}

#[tokio::test]
async fn a_loop_carries_its_variables_until_the_break_condition_holds() {
    for (satisfied_at, loop_count, rounds, reason) in [
        (1, 4, 1, WorkflowLoopExit::LoopBreak),
        (3, 4, 3, WorkflowLoopExit::LoopBreak),
        (9, 4, 4, WorkflowLoopExit::LoopCompleted),
    ] {
        let plane = workflow_test_plane(1);
        let runtime = workflow_runtime(&plane);
        let observations = runtime.observations.subscribe();
        let checker = checker(satisfied_at);
        let output = run(
            &plane,
            runtime,
            checker.clone(),
            checker_loop(loop_count),
            json!({"passed":false,"label":"initial"}),
        )
        .await
        .unwrap();
        assert_eq!(
            output,
            json!({"passed":reason == WorkflowLoopExit::LoopBreak,"label":format!("committed-{rounds}")})
        );
        assert_eq!(checker.starts.load(Ordering::SeqCst), rounds);
        let received = checker.received.lock().unwrap();
        for (index, call) in received.iter().enumerate() {
            // Each round reads the variables the previous round wrote.
            assert_eq!(
                call.arguments["label"],
                if index == 0 {
                    "initial".into()
                } else {
                    format!("committed-{index}")
                }
            );
            let ToolInvocationId::Workflow { node, .. } = &call.id else {
                panic!("native workflow identity")
            };
            assert_eq!(node.block.definition.blocks, ["feedback", "body"]);
            assert_eq!(
                node.block.invocations,
                [0, u32::try_from(index + 1).unwrap()]
            );
        }
        let events = observations.borrow();
        let admitted = count(&events, |event| {
            matches!(event, RuntimeEvent::WorkflowLoopIterationAdmitted { .. })
        });
        assert_eq!(admitted, rounds);
        let exits = events
            .iter()
            .filter_map(|event| match event {
                RuntimeEvent::WorkflowLoopExited {
                    iterations, status, ..
                } => Some((*iterations, *status)),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(exits, [(u32::try_from(rounds).unwrap(), reason)]);
        assert!(plane.registry.all_snapshots().is_empty());
    }
}

#[tokio::test]
async fn a_break_condition_that_already_holds_runs_no_round() {
    let mut definition = checker_loop(4);
    let WorkflowNodeDefinition::Loop { loop_variables, .. } =
        definition.block.nodes.get_mut("feedback").unwrap()
    else {
        unreachable!()
    };
    loop_variables[0].value = WorkflowValue::Literal { value: json!(true) };
    let plane = workflow_test_plane(1);
    let checker = checker(1);
    let output = run(
        &plane,
        workflow_runtime(&plane),
        checker.clone(),
        definition,
        json!({"passed":false,"label":"initial"}),
    )
    .await
    .unwrap();
    assert_eq!(output, json!({"passed":true,"label":"initial"}));
    assert_eq!(checker.starts.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn exit_loop_ends_the_loop_once_its_round_settles() {
    let definition = around(
        "feedback",
        &state(),
        &schema(json!({}), &[]),
        &json!({"type":"loop","loop_count":10,
            "body":{"nodes":{"start":{"type":"start"},
                "check":check_node(&path(&["args"])),
                "passed":{"type":"branch","cases":[{"id":"true","condition":{"type":"boolean","value":path(&["check","passed"])}}]},
                "stop":{"type":"loop_end"}},
                "edges":[{"from":"start","to":"check"},{"from":"check","to":"passed"},
                    {"from":"passed","to":"stop","port":"true"}]}}),
    );
    let plane = workflow_test_plane(1);
    let runtime = workflow_runtime(&plane);
    let observations = runtime.observations.subscribe();
    let checker = checker(2);
    let output = run(
        &plane,
        runtime,
        checker.clone(),
        definition,
        json!({"passed":false,"label":"initial"}),
    )
    .await
    .unwrap();
    assert_eq!(output, json!({}));
    assert_eq!(checker.starts.load(Ordering::SeqCst), 2);
    assert!(observations.borrow().iter().any(|event| matches!(
        event,
        RuntimeEvent::WorkflowLoopExited {
            iterations: 2,
            status: WorkflowLoopExit::LoopBreak,
            ..
        }
    )));
}

#[tokio::test]
async fn cancellation_at_the_second_round_starts_nothing_new() {
    let plane = workflow_test_plane(1);
    let runtime = workflow_runtime(&plane);
    let observations = runtime.observations.subscribe();
    let checker = checker(usize::MAX);
    let context = context_with_registration(
        &plane,
        ToolRegistration::plain(definition(), checker.clone()),
        crate::agent::AttemptLifecycle::default(),
    );
    let (entered, enter) = tokio::sync::oneshot::channel();
    let (release, wait) = tokio::sync::oneshot::channel();
    *runtime.iteration_frontier.lock().unwrap() = Some(execution::IterationFrontierHook {
        iteration: 2,
        entered,
        release: wait,
    });
    let (trigger, cancellation) = workflow_cancellation();
    let program = Arc::new(compile_test(checker_loop(3)).unwrap());
    let task = tokio::spawn(async move {
        runtime
            .run_test_foreground(
                program,
                ToolCallId::new("cancel-loop"),
                context,
                json!({"passed":false,"label":"initial"}),
                cancellation,
            )
            .await
    });
    enter.await.unwrap();
    trigger.cancel();
    release.send(()).unwrap();
    assert!(task.await.unwrap().unwrap_err().is_cancelled());
    assert_eq!(checker.starts.load(Ordering::SeqCst), 1);
    let events = observations.borrow();
    assert_eq!(
        count(&events, |event| matches!(
            event,
            RuntimeEvent::WorkflowLoopIterationAdmitted { .. }
        )),
        1
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, RuntimeEvent::WorkflowLoopExited { .. }))
    );
}

#[tokio::test]
async fn an_iteration_runs_its_body_once_per_item_in_order() {
    let plane = workflow_test_plane(1);
    let echo = Arc::new(Echo::default());
    let output = run(
        &plane,
        workflow_runtime(&plane),
        echo.clone(),
        echo_iteration(&json!({})),
        json!({"items":["a","b","c"]}),
    )
    .await
    .unwrap();
    assert_eq!(output, json!({"output":["a","b","c"]}));
    assert_eq!(echo.peak.load(Ordering::SeqCst), 1, "sequential by default");
    let empty = run(
        &plane,
        workflow_runtime(&plane),
        echo.clone(),
        echo_iteration(&json!({})),
        json!({"items":[]}),
    )
    .await
    .unwrap();
    assert_eq!(empty, json!({"output":[]}));
}

#[tokio::test]
async fn parallel_mode_runs_up_to_parallel_nums_rounds_at_once() {
    let plane = workflow_test_plane(1);
    let echo = Arc::new(Echo::default());
    let output = run(
        &plane,
        workflow_runtime(&plane),
        echo.clone(),
        echo_iteration(&json!({"is_parallel":true,"parallel_nums":2})),
        json!({"items":["a","b","c","d","e"]}),
    )
    .await
    .unwrap();
    assert_eq!(output, json!({"output":["a","b","c","d","e"]}));
    assert_eq!(echo.peak.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn error_modes_decide_what_a_failed_round_contributes() {
    for (mode, expected) in [
        ("continue_on_error", Some(json!(["a", null, "c"]))),
        ("remove_abnormal_output", Some(json!(["a", "c"]))),
        ("terminated", None),
    ] {
        let plane = workflow_test_plane(1);
        let echo = Arc::new(Echo::default());
        let result = run(
            &plane,
            workflow_runtime(&plane),
            echo.clone(),
            echo_iteration(&json!({"error_handle_mode":mode})),
            json!({"items":["a","bad","c"]}),
        )
        .await;
        match expected {
            Some(output) => assert_eq!(result.unwrap(), json!({"output":output}), "{mode}"),
            None => assert!(!result.unwrap_err().is_cancelled(), "{mode}"),
        }
    }
}

fn body(definition: &mut WorkflowDefinition) -> &mut WorkflowBlock {
    match definition.block.nodes.get_mut("feedback").unwrap() {
        WorkflowNodeDefinition::Loop { body, .. } => body,
        _ => unreachable!(),
    }
}

#[test]
fn bodies_follow_difys_rules_at_compile() {
    assert!(compile_test(checker_loop(3)).is_ok());
    assert!(compile_test(echo_iteration(&json!({}))).is_ok());
    let rejected = |definition: WorkflowDefinition, why: &str| {
        assert!(compile_test(definition).is_err(), "{why}");
    };
    for loop_count in [0, LOOP_NODE_MAX_COUNT + 1] {
        let mut definition = checker_loop(3);
        let WorkflowNodeDefinition::Loop {
            loop_count: count, ..
        } = definition.block.nodes.get_mut("feedback").unwrap()
        else {
            unreachable!()
        };
        *count = loop_count;
        rejected(definition, "loop_count out of range");
    }
    let save = |operation: &str, value: Option<Value>| {
        let mut definition = checker_loop(3);
        let mut item = json!({"variable":["feedback","label"],"operation":operation});
        if let Some(value) = value {
            item["value"] = value;
        }
        body(&mut definition).nodes.insert(
            "save".into(),
            serde_json::from_value(json!({"type":"assigner","items":[item]})).unwrap(),
        );
        definition
    };
    assert!(compile_test(save("clear", None)).is_ok());
    rejected(
        save("increment", Some(json!({"type":"literal","value":1}))),
        "+= on a string",
    );
    rejected(
        save("append", Some(json!({"type":"literal","value":"x"}))),
        "append to a string",
    );
    rejected(save("set", None), "set without a value");
    rejected(
        save("clear", Some(json!({"type":"literal","value":""}))),
        "clear with a value",
    );
    let mut outside = checker_loop(3);
    let save_node = body(&mut outside).nodes.remove("save").unwrap();
    body(&mut outside).edges.pop();
    outside.block.nodes.insert("save".into(), save_node);
    outside.block.edges = vec![
        edge("workflow_start", "save"),
        edge("save", "feedback"),
        edge("feedback", "done"),
    ];
    rejected(outside, "an Assigner outside its Loop");
    let mut stop = checker_loop(3);
    stop.block
        .nodes
        .insert("stop".into(), WorkflowNodeDefinition::LoopEnd {});
    stop.block.edges.push(edge("done", "stop"));
    rejected(stop, "Exit Loop at the root");
    let mut returns = checker_loop(3);
    body(&mut returns).nodes.insert(
        "back".into(),
        WorkflowNodeDefinition::Return {
            output: reference("check"),
        },
    );
    body(&mut returns).edges.push(edge("save", "back"));
    rejected(returns, "Return in a body");
    let mut shadowed = checker_loop(3);
    let check = body(&mut shadowed).nodes["check"].clone();
    body(&mut shadowed).nodes.insert("feedback".into(), check);
    body(&mut shadowed).edges.push(edge("save", "feedback"));
    rejected(shadowed, "a body ID hides a node around it");
    let mut declared = checker_loop(3);
    body(&mut declared).input = state();
    rejected(declared, "a body declares input");
    rejected(
        echo_iteration(&json!({"iterator":{"type":"literal","value":["a"]}})),
        "a literal iterator",
    );
    rejected(
        echo_iteration(&json!({"is_parallel":true,"parallel_nums":MAX_PARALLEL_LIMIT + 1})),
        "parallel_nums out of range",
    );
}

#[test]
fn an_iteration_output_follows_its_rounds_value() {
    let output = |extra: Value| {
        compile_test(echo_iteration(&extra))
            .unwrap()
            .node_outputs()
            .remove("block.nodes.each")
            .unwrap()
    };
    assert_eq!(
        output(json!({})),
        json!({"type":"object","properties":{"output":{"type":"array","items":{"type":"string"}}},"required":["output"],"additionalProperties":false})
    );
    assert_eq!(
        output(json!({"error_handle_mode":"continue_on_error"}))["properties"]["output"],
        json!({"type":"array"})
    );
}
