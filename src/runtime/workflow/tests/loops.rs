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
        "input":input,"nodes":{"workflow_start":{"type":"start"},id:container,
            "done":{"type":"return","outputs":output["properties"].as_object().unwrap().keys()
                .map(|key| json!({"variable":key,"value_selector":[id,key]})).collect::<Vec<_>>()}},
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
            outputs: Vec::new(),
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

/// Each admitted Tool reports entry and waits on its own gate. Neither
/// completion order nor the Workflow's terminal boundary depends on sleeps.
struct RoundGates {
    entered: tokio::sync::mpsc::UnboundedSender<usize>,
    gates: [tokio::sync::Semaphore; 2],
    finished: AtomicUsize,
}
impl ToolExecutor for RoundGates {
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
                let index = usize::from(invocation.arguments["label"] == "unknown");
                self.entered.send(index).unwrap();
                self.gates[index].acquire().await.unwrap().forget();
                self.finished.fetch_add(1, Ordering::SeqCst);
                terminal(if index == 0 {
                    ToolExecutionStatus::Failed {
                        error: "known failure".into(),
                    }
                } else {
                    ToolExecutionStatus::OutcomeUnknown {
                        detail: "uncertain effect".into(),
                    }
                })
            }),
            context.cancellation,
        )
    }
}

#[tokio::test]
async fn parallel_round_failures_preserve_uncertainty_in_both_completion_orders() {
    for first in [0, 1] {
        let plane = workflow_test_plane(1);
        let runtime = workflow_runtime(&plane);
        let read_model = runtime.read_model.clone();
        let mut changes = read_model.subscribe();
        let (entered, mut entries) = tokio::sync::mpsc::unbounded_channel();
        let gates = Arc::new(RoundGates {
            entered,
            gates: std::array::from_fn(|_| tokio::sync::Semaphore::new(0)),
            finished: AtomicUsize::new(0),
        });
        let task = run(
            &plane,
            runtime,
            gates.clone(),
            echo_iteration(&json!({"is_parallel":true,"parallel_nums":2})),
            json!({"items":["failed","unknown"]}),
        );
        tokio::pin!(task);
        for _ in 0..2 {
            tokio::select! { entry = entries.recv() => { entry.unwrap(); }, _ = &mut task => panic!("both Tools must enter") }
        }
        gates.gates[first].add_permits(1);
        loop {
            tokio::select! {
                _ = &mut task => panic!("terminal before the other admitted Tool settled"),
                update = changes.changed() => { update.unwrap(); }
            }
            if gates.finished.load(Ordering::SeqCst) == 1 {
                break;
            }
        }
        assert_eq!(gates.finished.load(Ordering::SeqCst), 1);
        gates.gates[1 - first].add_permits(1);
        let error = task.await.unwrap_err();
        assert_eq!(gates.finished.load(Ordering::SeqCst), 2);
        assert!(matches!(
            error.execution_status(),
            ToolExecutionStatus::OutcomeUnknown { .. }
        ));
        assert!(matches!(
            read_model.snapshot().runs[0].state,
            crate::runtime::workflow::read_model::WorkflowState::Settled {
                outcome: WorkflowExecutionOutcome::OutcomeUnknown
            }
        ));
    }
}

#[test]
fn iteration_exports_require_activation_on_every_round_completion() {
    let mut definition = echo_iteration(&json!({}));
    let WorkflowNodeDefinition::Iteration { body, .. } =
        definition.block.nodes.get_mut("each").unwrap()
    else {
        unreachable!()
    };
    body.nodes.insert("choose".into(), serde_json::from_value(json!({"type":"branch","cases":[{"id":"yes","condition":{"type":"boolean","value":{"type":"literal","value":true}}}]})).unwrap());
    body.edges = vec![
        edge("start", "choose"),
        WorkflowEdgeDefinition {
            from: "choose".into(),
            to: "check".into(),
            port: Some(WorkflowPort::case("yes")),
        },
    ];
    assert!(
        compile_test(definition).is_err(),
        "the unconnected ELSE still completes a round"
    );
    assert!(compile_test(echo_iteration(&json!({}))).is_ok());
}

#[test]
fn dynamic_execution_bounds_are_invariant_under_node_order_and_nesting() {
    for id in ["aloop", "zloop"] {
        let mut definition = echo_iteration(&json!({}));
        definition.block.nodes.insert(id.into(), serde_json::from_value(json!({"type":"loop","loop_count":2,"loop_variables":[],"body":{"nodes":{"inner_start":{"type":"start"}},"edges":[]}})).unwrap());
        definition.block.edges = vec![
            edge("workflow_start", "each"),
            edge("each", id),
            edge(id, "done"),
        ];
        let program = compile_test(definition).unwrap();
        assert_eq!(program.execution_bound, MAX_WORKFLOW_STEPS);
    }
    let mut each = echo_iteration(&json!({}))
        .block
        .nodes
        .remove("each")
        .unwrap();
    let WorkflowNodeDefinition::Iteration {
        body: inner,
        output,
        ..
    } = &mut each
    else {
        unreachable!()
    };
    let check = inner.nodes.remove("check").unwrap();
    inner.nodes.insert("inner_check".into(), check);
    inner.edges = vec![edge("start", "inner_check")];
    *output = serde_json::from_value(path(&["inner_check", "label"])).unwrap();
    let mut definition = checker_loop(2);
    body(&mut definition).nodes.insert("each".into(), each);
    body(&mut definition).edges.push(edge("save", "each"));
    definition
        .block
        .input
        .as_object_mut()
        .unwrap()
        .get_mut("properties")
        .unwrap()
        .as_object_mut()
        .unwrap()
        .insert(
            "items".into(),
            json!({"type":"array","items":{"type":"string"},"default":[]}),
        );
    assert_eq!(
        compile_test(definition).unwrap().execution_bound,
        MAX_WORKFLOW_STEPS
    );
}

#[tokio::test]
async fn accumulated_iteration_exports_remain_reserved_and_stop_new_rounds() {
    let mut definition = echo_iteration(&json!({}));
    definition.block.input = schema(
        json!({"items":{"type":"array","items":{"type":"boolean"}},"text":{"type":"string"}}),
        &["items", "text"],
    );
    let WorkflowNodeDefinition::Iteration { body, output, .. } =
        definition.block.nodes.get_mut("each").unwrap()
    else {
        unreachable!()
    };
    body.nodes.retain(|id, _| id == "start");
    body.edges.clear();
    *output = serde_json::from_value(path(&["args", "text"])).unwrap();
    let plane = workflow_test_plane(1);
    let runtime = workflow_runtime(&plane);
    let observations = runtime.observations.subscribe();
    let error = run(
        &plane,
        runtime,
        Arc::new(Echo::default()),
        definition,
        json!({"items":vec![true; 1000],"text":"x".repeat(50_000)}),
    )
    .await
    .unwrap_err();
    assert!(matches!(
        error,
        WorkflowRunError::LimitExceeded(WorkflowLimit::RetainedData)
    ));
    let rounds = count(&observations.borrow(), |event| {
        matches!(event, RuntimeEvent::WorkflowLoopIterationSettled { .. })
    });
    assert!(
        rounds < 1000,
        "retention fails during accumulation, not at final export"
    );
}

#[tokio::test]
async fn continued_rounds_never_swallow_native_call_admission_limits() {
    for mode in ["continue_on_error", "remove_abnormal_output"] {
        for (kind, limit) in [
            ("model", WorkflowLimit::ModelCalls),
            ("agent", WorkflowLimit::Agents),
        ] {
            let leaf = if kind == "model" {
                json!({"type":"model","model":{"mode":"session"},"messages":[{"role":"system","content":[{"type":"literal","value":"Follow the task."}]},{"role":"user","content":[{"type":"literal","value":"go"}]}],"output":{"type":"text"}})
            } else {
                json!({"type":"agent","profile":"reviewer","task":[{"type":"literal","value":"go"}],"output":{"type":"json_schema","schema":state()}})
            };
            let mut definition = echo_iteration(&json!({"error_handle_mode":mode}));
            let WorkflowNodeDefinition::Iteration { body, output, .. } =
                definition.block.nodes.get_mut("each").unwrap()
            else {
                unreachable!()
            };
            body.nodes.retain(|id, _| id == "start");
            for id in ["a", "b"] {
                body.nodes
                    .insert(id.into(), serde_json::from_value(leaf.clone()).unwrap());
            }
            body.edges = vec![edge("start", "a"), edge("start", "b")];
            *output = serde_json::from_value(path(&["each", "item"])).unwrap();
            let program = Arc::new(compile_test(definition).unwrap());
            let run = WorkflowRun::new(
                program.clone(),
                test_instance("test_workflow", "each").block.run,
            );
            run.budgets.lock().unwrap().exhaust(limit);
            let plane = workflow_test_plane(1);
            let runtime = workflow_runtime(&plane);
            let context = workflow_test_context(&plane);
            let (_, cancellation) = workflow_cancellation();
            let result = runtime
                .execute_block(
                    &run,
                    &program.block,
                    &context,
                    json!({"items":["x"]}),
                    vec![0],
                    &cancellation,
                )
                .await;
            let Err(error) = result else {
                panic!("{mode} swallowed {limit:?}")
            };
            // The first rejected admission closes the graph's gate; its
            // queued sibling is never admitted after that failure.
            assert_eq!(error, WorkflowRunError::LimitExceeded(limit));
        }
    }
}

#[tokio::test]
async fn recursive_default_expansion_stops_before_any_graph_or_native_child_starts() {
    let input = (0..10).fold(schema(json!({}), &[]), |child, _| {
        schema(
            json!({"rows":{"type":"array","items":child,"default":vec![json!({});8]}}),
            &[],
        )
    });
    let definition = serde_json::from_value(json!({"description":"Bound default expansion","block":{
        "input":input,"nodes":{"start":{"type":"start"},
        "check":check_node(&json!({"type":"literal","value":{"passed":true,"label":"unused"}})),
        "done":{"type":"return","outputs":[]}},"edges":[{"from":"start","to":"check"},{"from":"check","to":"done"}]}})).unwrap();
    let plane = workflow_test_plane(1);
    let runtime = workflow_runtime(&plane);
    let observations = runtime.observations.subscribe();
    let executor = Arc::new(Echo::default());
    let result = run(&plane, runtime, executor.clone(), definition, json!({})).await;
    assert!(matches!(
        result,
        Err(WorkflowRunError::LimitExceeded(WorkflowLimit::RetainedData))
    ));
    assert_eq!(executor.starts.load(Ordering::SeqCst), 0);
    assert!(
        !observations
            .borrow()
            .iter()
            .any(|event| matches!(event, RuntimeEvent::WorkflowNodeStarted { .. })),
        "no graph node, hence no Tool, Model or Agent can start"
    );
}

fn assigned_loop(items: &Value) -> WorkflowDefinition {
    around(
        "repeat",
        &schema(json!({"payload":{"type":"string"}}), &["payload"]),
        &schema(
            json!({"xs":{"type":"array","items":{"type":"string"}}}),
            &["xs"],
        ),
        &json!({"type":"loop","loop_count":1,"loop_variables":[
            {"label":"xs","var_type":"array[string]","value":{"type":"literal","value":[]}},
            {"label":"n","var_type":"number","value":{"type":"literal","value":1}}],
            "body":{"nodes":{"start":{"type":"start"},"save":{"type":"assigner","items":items}},
            "edges":[{"from":"start","to":"save"}]}}),
    )
}

#[tokio::test]
async fn append_then_clear_cannot_hide_intermediate_assignment_budget_overflow() {
    let mut items = vec![
        json!({"variable":["repeat","xs"],"operation":"append","value":path(&["args","payload"])});
        31
    ];
    items.push(json!({"variable":["repeat","xs"],"operation":"clear"}));
    let definition = assigned_loop(&json!(items));
    let plane = workflow_test_plane(1);
    let executor = checker(1);
    let result = run(
        &plane,
        workflow_runtime(&plane),
        executor.clone(),
        definition,
        json!({"payload":"x".repeat(50_000)}),
    )
    .await;
    assert_eq!(
        result,
        Err(WorkflowRunError::LimitExceeded(WorkflowLimit::RetainedData))
    );
    assert_eq!(executor.starts.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn observed_failure_closes_admission_before_a_queued_native_sibling() {
    let mut definition = assigned_loop(&json!([]));
    let WorkflowNodeDefinition::Loop { body: block, .. } =
        definition.block.nodes.get_mut("repeat").unwrap()
    else {
        unreachable!()
    };
    block.nodes.remove("save");
    block.nodes.insert("a_error".into(), serde_json::from_value(json!({"type":"assigner","items":[{"variable":["repeat","n"],"operation":"divide","value":{"type":"literal","value":0}}]})).unwrap());
    block.nodes.insert(
        "z_write".into(),
        serde_json::from_value(check_node(
            &json!({"type":"literal","value":{"passed":true,"label":"must not start"}}),
        ))
        .unwrap(),
    );
    block.edges = vec![edge("start", "a_error"), edge("start", "z_write")];
    let plane = workflow_test_plane(1);
    let runtime = workflow_runtime(&plane);
    let observations = runtime.observations.subscribe();
    let (entered, before_admission) = tokio::sync::oneshot::channel();
    let (release, gate) = tokio::sync::oneshot::channel();
    *runtime.node_frontier.lock().unwrap() = Some(execution::NodeFrontierHook {
        node: "z_write".into(),
        entered,
        release: gate,
    });
    let executor = checker(1);
    let task = tokio::spawn({
        let executor = executor.clone();
        async move {
            run(
                &plane,
                runtime,
                executor,
                definition,
                json!({"payload":"x"}),
            )
            .await
        }
    });
    before_admission.await.unwrap();
    assert_eq!(executor.starts.load(Ordering::SeqCst), 0);
    assert!(!observations.borrow().iter().any(|event| matches!(event, RuntimeEvent::WorkflowNodeStarted {instance} if instance.node == "z_write")));
    release.send(()).unwrap();
    assert!(task.await.unwrap().is_err());
    assert_eq!(executor.starts.load(Ordering::SeqCst), 0);
}

#[test]
fn unordered_same_variable_writers_require_an_edge_or_disjoint_branches() {
    let mut definition = assigned_loop(&json!([{"variable":["repeat","xs"],"operation":"clear"}]));
    let WorkflowNodeDefinition::Loop { body: block, .. } =
        definition.block.nodes.get_mut("repeat").unwrap()
    else {
        unreachable!()
    };
    block
        .nodes
        .insert("other".into(), block.nodes["save"].clone());
    block.edges.push(edge("start", "other"));
    assert!(
        compile_test(definition.clone())
            .unwrap_err()
            .to_string()
            .contains("without an execution order")
    );
    let WorkflowNodeDefinition::Loop { body: block, .. } =
        definition.block.nodes.get_mut("repeat").unwrap()
    else {
        unreachable!()
    };
    block.edges = vec![edge("start", "save"), edge("save", "other")];
    assert!(compile_test(definition.clone()).is_ok());
    let WorkflowNodeDefinition::Loop { body: block, .. } =
        definition.block.nodes.get_mut("repeat").unwrap()
    else {
        unreachable!()
    };
    block.nodes.insert("choose".into(),serde_json::from_value(json!({"type":"branch","cases":[{"id":"yes","condition":{"type":"boolean","value":{"type":"literal","value":true}}}]})).unwrap());
    block.edges = vec![
        edge("start", "choose"),
        branch_edge("choose", "save", WorkflowPort::case("yes")),
        branch_edge("choose", "other", WorkflowPort::False),
    ];
    assert!(compile_test(definition).is_ok());
}

#[test]
fn root_native_nodes_require_success_exits_but_bodies_may_finish_naturally() {
    let nodes = [
        json!({"type":"model","model":{"mode":"session"},"messages":[{"role":"system","content":[{"type":"literal","value":"Hello"}]},{"role":"user","content":[{"type":"literal","value":"Hello"}]}],"output":{"type":"text"},"on_error":{"type":"fail_branch"}}),
        json!({"type":"tool","selector":{"origin":"builtin","name":"check"},"arguments":{"type":"literal","value":{}},"result":{"type":"text"},"on_error":{"type":"fail_branch"}}),
        json!({"type":"agent","profile":"reviewer","task":[{"type":"literal","value":"Hello"}],"output":{"type":"text"},"on_error":{"type":"fail_branch"}}),
    ];
    for node in nodes {
        let mut definition: WorkflowDefinition = serde_json::from_value(json!({"description":"All semantic exits","block":{"input":schema(json!({}), &[]),"nodes":{"start":{"type":"start"},"work":node,"done":{"type":"return","outputs":[]}},"edges":[{"from":"start","to":"work"},{"from":"work","to":"done","port":"fail"}]}})).unwrap();
        assert!(
            compile_test(definition.clone())
                .unwrap_err()
                .to_string()
                .contains("success (next)")
        );
        definition.block.edges.push(edge("work", "done"));
        assert!(compile_test(definition).is_ok());
        let mut definition =
            assigned_loop(&json!([{"variable":["repeat","xs"],"operation":"clear"}]));
        let WorkflowNodeDefinition::Loop { body, .. } =
            definition.block.nodes.get_mut("repeat").unwrap()
        else {
            unreachable!()
        };
        body.nodes
            .insert("work".into(), serde_json::from_value(node).unwrap());
        body.edges = vec![
            edge("start", "work"),
            branch_edge("work", "save", WorkflowPort::Fail),
        ];
        assert!(compile_test(definition).is_ok());
    }
}

#[tokio::test]
async fn many_ordered_boolean_assigners_remain_within_their_small_value_budget() {
    let mut definition = around(
        "repeat",
        &schema(json!({}), &[]),
        &schema(json!({"flag":{"type":"boolean"}}), &["flag"]),
        &json!({"type":"loop","loop_count":1,"loop_variables":[{"label":"flag","var_type":"boolean","value":{"type":"literal","value":false}}],"body":{"nodes":{"start":{"type":"start"}},"edges":[]}}),
    );
    let WorkflowNodeDefinition::Loop { body, .. } =
        definition.block.nodes.get_mut("repeat").unwrap()
    else {
        unreachable!()
    };
    let mut previous = "start".to_owned();
    for index in 0..31 {
        let id = format!("write_{index}");
        body.nodes.insert(id.clone(),serde_json::from_value(json!({"type":"assigner","items":[{"variable":["repeat","flag"],"operation":"overwrite","value":{"type":"literal","value":true}}]})).unwrap());
        body.edges.push(edge(&previous, &id));
        previous = id;
    }
    let program = compile_test(definition.clone()).unwrap();
    assert!(program.retained_bound < MAX_VALUE_BYTES);
    let plane = workflow_test_plane(1);
    let result = run(
        &plane,
        workflow_runtime(&plane),
        checker(1),
        definition,
        json!({}),
    )
    .await
    .unwrap();
    assert_eq!(result, json!({"flag": true}));
}

fn merged_assignment_loop(parallel: bool) -> WorkflowDefinition {
    let edges = if parallel {
        json!([{"from":"start","to":"b_x"},{"from":"start","to":"c_y"},{"from":"b_x","to":"z_effect"},{"from":"c_y","to":"z_effect"}])
    } else {
        json!([{"from":"start","to":"b_x"},{"from":"b_x","to":"c_y"},{"from":"c_y","to":"z_effect"}])
    };
    around(
        "repeat",
        &schema(json!({"payload":{"type":"string"}}), &["payload"]),
        &schema(json!({}), &[]),
        &json!({"type":"loop","loop_count":1,"loop_variables":[
            {"label":"x","var_type":"string","value":{"type":"literal","value":""}},
            {"label":"y","var_type":"string","value":{"type":"literal","value":""}}],
            "body":{"nodes":{"start":{"type":"start"},
                "b_x":{"type":"assigner","items":[{"variable":["repeat","x"],"operation":"set","value":path(&["args","payload"])}]},
                "c_y":{"type":"assigner","items":[{"variable":["repeat","y"],"operation":"set","value":path(&["args","payload"])}]},
                "z_effect":check_node(&json!({"type":"literal","value":{"passed":true,"label":"effect"}}))},
                "edges":edges}}),
    )
}

#[tokio::test]
async fn merged_assignment_owners_are_bounded_before_successor_admission() {
    for parallel in [false, true] {
        for bytes in [30_000, 40_000] {
            let plane = workflow_test_plane(1);
            let runtime = workflow_runtime(&plane);
            let observations = runtime.observations.subscribe();
            let executor = checker(1);
            let result = run(
                &plane,
                runtime,
                executor.clone(),
                merged_assignment_loop(parallel),
                json!({"payload":"x".repeat(bytes)}),
            )
            .await;
            let started=observations.borrow().iter().any(|event| matches!(event,RuntimeEvent::WorkflowNodeStarted {instance} if instance.node=="z_effect"));
            if bytes == 40_000 {
                assert_eq!(
                    result,
                    Err(WorkflowRunError::LimitExceeded(WorkflowLimit::RetainedData)),
                    "parallel={parallel}"
                );
                assert_eq!(executor.starts.load(Ordering::SeqCst), 0);
                assert!(!started);
            } else {
                assert_eq!(result.unwrap(), json!({}));
                assert_eq!(executor.starts.load(Ordering::SeqCst), 1);
                assert!(started);
            }
        }
    }
}

#[tokio::test]
async fn a_merge_budget_failure_drains_admitted_native_work_and_closes_queued_admission() {
    let mut definition = merged_assignment_loop(true);
    let WorkflowNodeDefinition::Loop { body, .. } =
        definition.block.nodes.get_mut("repeat").unwrap()
    else {
        unreachable!()
    };
    let tool = json!({"type":"tool","selector":{"origin":"builtin","name":"check"},"arguments":{"type":"literal","value":{"passed":true,"label":"drain"}},"result":{"type":"text"}});
    for id in ["a_inflight", "z_queued"] {
        body.nodes
            .insert(id.into(), serde_json::from_value(tool.clone()).unwrap());
        body.edges.push(edge("start", id));
    }
    let plane = workflow_test_plane(1);
    let runtime = workflow_runtime(&plane);
    let observations = runtime.observations.subscribe();
    let (entered, queued) = tokio::sync::oneshot::channel();
    let (release, gate) = tokio::sync::oneshot::channel();
    *runtime.node_frontier.lock().unwrap() = Some(execution::NodeFrontierHook {
        node: "z_queued".into(),
        entered,
        release: gate,
    });
    let mut executor = Probe::new(ToolExecutionStatus::Success);
    let native_release = Arc::new(tokio::sync::Notify::new());
    Arc::get_mut(&mut executor).unwrap().release = Some(native_release.clone());
    let mut started = executor.started.subscribe();
    let task = tokio::spawn({
        let executor = executor.clone();
        async move {
            run(
                &plane,
                runtime,
                executor,
                definition,
                json!({"payload":"x".repeat(40_000)}),
            )
            .await
        }
    });
    queued.await.unwrap(); // First polled while draining after c_y's failed merge.
    while *started.borrow() == 0 {
        started.changed().await.unwrap();
    }
    assert!(!task.is_finished());
    release.send(()).unwrap();
    assert!(
        !task.is_finished(),
        "the admitted native owner still has to settle"
    );
    native_release.notify_one();
    assert_eq!(
        task.await.unwrap(),
        Err(WorkflowRunError::LimitExceeded(WorkflowLimit::RetainedData))
    );
    assert_eq!(executor.starts.load(Ordering::SeqCst), 1);
    assert!(!observations.borrow().iter().any(|event| matches!(event,RuntimeEvent::WorkflowNodeStarted {instance} if ["z_effect","z_queued"].contains(&instance.node.as_str()))));
}
