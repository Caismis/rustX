//! Issue #462 Workflow regressions over the shared Session workspace.
//!
//! Workflow Agents and Tools act directly in the frozen Session execution
//! binding: there is no Candidate, worktree, freeze or rollback. Review is a
//! typed `{accepted, feedback}` value that gates later work only through an
//! explicit Branch. Every proof uses the real registry, real native Bash and
//! the native Review owner; ordering is driven by IPC frames and interaction
//! publications, never by elapsed time.
use super::human::{answer, owner};
use super::*;

const IMPLEMENT_FILE: &str = "greeting.txt";

fn empty_object() -> Value {
    schema(json!({}), &[])
}

fn implement_node() -> Value {
    json!({"type":"agent","profile":"reviewer","task":"Implement the fixed greeting change.",
        "input":{},"output":empty_object()})
}

fn check_report() -> Value {
    schema(
        json!({"exit_code":{"type":"integer"},"stdout":{"type":"string"},
            "stderr":{"type":"string"},"combined":{"type":"string"}}),
        &["exit_code", "stdout", "stderr", "combined"],
    )
}

/// A frozen native Bash checker that reads the Implementer's file from the
/// shared workspace. Exit zero means the check ran; stdout carries its verdict.
fn check_node() -> Value {
    json!({"type":"tool","selector":{"origin":"builtin","name":"bash"},
        "arguments":{"type":"literal","value":{"command":
            format!("if [ \"$(cat {IMPLEMENT_FILE})\" = ok ]; then printf passed; else printf failed; fi")}},
        "result":{"type":"json","part":0,"schema":check_report()}})
}

fn files_named(root: &std::path::Path, name: &str) -> Vec<std::path::PathBuf> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir(root).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if entry.file_type().unwrap().is_dir() {
            found.extend(files_named(&path, name));
        } else if entry.file_name() == name {
            found.push(path);
        }
    }
    found
}

fn bash_context(
    plane: &WorkflowTestPlane,
    owner: Option<Arc<crate::runtime::interaction::InteractionCoordinator>>,
) -> crate::runtime::subagent::NativeExecutionContext {
    let lifecycle = crate::agent::AttemptLifecycle::default().with_pre_tool_policy(Arc::new(
        crate::agent::lifecycle::ConfiguredApprovalPolicy::new(ApprovalMode::FullAccess),
    ));
    let lifecycle = match owner {
        Some(owner) => lifecycle.with_native_interaction(owner),
        None => lifecycle,
    };
    context_with_registration(
        plane,
        crate::tools::native::test_bash_registration(),
        lifecycle,
    )
}

/// C01/C05/C06/C07/C12: Implement -> Check failure -> Repair -> Check
/// success over a plain (non-Git) workspace. Each Workflow Agent runs in the
/// exact Session binding, native Bash observes its writes in place, the
/// outer result is exactly the declared Loop output, and the completed
/// writes remain after settlement.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn implement_check_repair_runs_in_the_shared_session_workspace() {
    let plane = workflow_test_plane(1);
    let workspace = plane.dir.path().join("workspace");
    let mut children = [stage_workflow_child(&plane), stage_workflow_child(&plane)];
    let context = bash_context(&plane, None);
    let passed = schema(json!({"passed":{"type":"boolean"}}), &["passed"]);
    let definition = serde_json::from_value(json!({"description":"shared implement/check","timeout_ms":600_000,"block":{
        "input":passed,"output":passed,"entry":"implement","nodes":{
            "implement":implement_node(),
            "check":check_node(),
            "verdict":{"type":"branch","condition":{"type":"equal",
                "left":{"type":"reference","path":["check","stdout"]},
                "right":{"type":"literal","value":"passed"}}},
            "pass":{"type":"return","output":{"type":"literal","value":{"passed":true}}},
            "fail":{"type":"return","output":{"type":"literal","value":{"passed":false}}}
        },"edges":[{"from":"implement","to":"check"},{"from":"check","to":"verdict"},
            {"from":"verdict","to":"pass","port":"true"},{"from":"verdict","to":"fail","port":"false"}]}}))
    .unwrap();
    let program = Arc::new(compile_test(super::loops::wrap_definition(definition, 3)).unwrap());
    let runtime = workflow_runtime(&plane);
    let observations = runtime.observations.subscribe();
    let (_, cancellation) = workflow_cancellation();
    let mut task = tokio::spawn(async move {
        runtime
            .run_test_foreground(
                program,
                ToolCallId::new("shared-repair"),
                context,
                json!({"passed":false}),
                cancellation,
            )
            .await
    });
    for (iteration, (child, content)) in children.iter_mut().zip(["wrong", "ok"]).enumerate() {
        tokio::select! {
            () = child.expect_delegate() => {},
            result = &mut task => panic!("run stopped before the writer: {result:?}"),
        }
        let snapshot = plane.registry.all_snapshots().pop().unwrap();
        assert_eq!(
            snapshot.workspace, workspace,
            "every Workflow Agent runs in the exact Session binding"
        );
        assert_eq!(
            observations
                .borrow()
                .iter()
                .filter(|e| matches!(e, RuntimeEvent::WorkflowLoopIterationSettled { .. }))
                .count(),
            iteration,
            "the repair writer starts only after the previous check settled"
        );
        std::fs::write(workspace.join(IMPLEMENT_FILE), content).unwrap();
        child
            .send_result(
                crate::runtime::subagent::ipc::ChildResultStatus::Succeeded,
                Some("{}"),
            )
            .await;
    }
    let output = task.await.unwrap().unwrap();
    assert_eq!(
        output,
        json!({"status":"satisfied","iterations":2,"result":{"passed":true}}),
        "the outer result is exactly the declared output, never wrapped"
    );
    assert_eq!(
        std::fs::read_to_string(workspace.join(IMPLEMENT_FILE)).unwrap(),
        "ok"
    );
    assert!(!workspace.join(".git").exists(), "no Git was ever required");
    assert_eq!(
        files_named(plane.dir.path(), IMPLEMENT_FILE),
        vec![workspace.join(IMPLEMENT_FILE)],
        "no worktree, copy or Candidate holds another version of the work"
    );
    assert!(plane.registry.unsettled_snapshot().is_empty());
}

/// C03/C04/C17: Review is ordinary typed data. A rejected plan takes the
/// explicit Branch's rejected Return and never admits the gated Agent; an
/// accepted plan reaches it. Neither path grants unrelated Tool authority.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn explicit_review_branch_gates_implementation() {
    for accepted in [false, true] {
        let plane = workflow_test_plane(1);
        let workspace = plane.dir.path().join("workspace");
        let (owner, _audit, mut published) = owner(&plane);
        let context = bash_context(&plane, Some(owner.clone()));
        let output = schema(
            json!({"status":{"type":"string","enum":["implemented","rejected"]},
                "feedback":{"type":"string"}}),
            &["status", "feedback"],
        );
        let definition = serde_json::from_value(json!({"description":"gated implementation","timeout_ms":600_000,"block":{
            "input":schema(json!({"summary":{"type":"string"}}), &["summary"]),"output":output,"entry":"review","nodes":{
                "review":{"type":"review","subject":{"type":"plan","value":{"type":"reference","path":["args"]}},"context":[]},
                "gate":{"type":"branch","condition":{"type":"boolean","value":{"type":"reference","path":["review","accepted"]}}},
                "implement":implement_node(),
                "implemented":{"type":"return","output":{"type":"object","fields":{
                    "status":{"type":"literal","value":"implemented"},
                    "feedback":{"type":"reference","path":["review","feedback"]}}}},
                "rejected":{"type":"return","output":{"type":"object","fields":{
                    "status":{"type":"literal","value":"rejected"},
                    "feedback":{"type":"reference","path":["review","feedback"]}}}}
            },"edges":[{"from":"review","to":"gate"},{"from":"gate","to":"implement","port":"true"},
                {"from":"gate","to":"rejected","port":"false"},{"from":"implement","to":"implemented"}]}}))
        .unwrap();
        let program = Arc::new(compile_test(definition).unwrap());
        let mut child = accepted.then(|| stage_workflow_child(&plane));
        let runtime = workflow_runtime(&plane);
        let (_, cancellation) = workflow_cancellation();
        let mut task = tokio::spawn(async move {
            runtime
                .run_test_foreground(
                    program,
                    ToolCallId::new("gated"),
                    context,
                    json!({"summary":"Write greeting.txt"}),
                    cancellation,
                )
                .await
        });
        let request = tokio::select! {
            result = &mut task => panic!("Workflow settled before Review: {result:?}"),
            request = published.recv() => request.unwrap(),
        };
        assert!(
            plane.registry.all_snapshots().is_empty(),
            "Review precedes and gates every implementation start"
        );
        owner
            .respond_async(&request.id, answer(&request, accepted))
            .await
            .unwrap();
        if let Some(child) = child.as_mut() {
            child.expect_delegate().await;
            std::fs::write(workspace.join(IMPLEMENT_FILE), "ok").unwrap();
            child
                .send_result(
                    crate::runtime::subagent::ipc::ChildResultStatus::Succeeded,
                    Some("{}"),
                )
                .await;
        }
        let value = task.await.unwrap().unwrap();
        if accepted {
            assert_eq!(value["status"], "implemented");
            assert_eq!(plane.registry.all_snapshots().len(), 1);
            assert!(workspace.join(IMPLEMENT_FILE).exists());
        } else {
            assert_eq!(
                value,
                json!({"status":"rejected","feedback":"Revise scope"}),
                "rejection is normal typed data routed by the explicit Branch"
            );
            assert!(
                plane.registry.all_snapshots().is_empty(),
                "the rejected Branch never admits gated implementation work"
            );
            assert!(!workspace.join(IMPLEMENT_FILE).exists());
        }
    }
}

/// C09: a Review after a completed write is only data. Rejecting it never
/// undoes the write, and the Workflow still settles through its declared
/// Return.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rejection_after_a_write_never_rolls_it_back() {
    let plane = workflow_test_plane(1);
    let workspace = plane.dir.path().join("workspace");
    let (owner, _audit, mut published) = owner(&plane);
    let context = bash_context(&plane, Some(owner.clone()));
    let mut child = stage_workflow_child(&plane);
    let definition = serde_json::from_value(json!({"description":"post-write review","timeout_ms":600_000,"block":{
        "input":schema(json!({"changed":{"type":"string"}}), &["changed"]),
        "output":crate::runtime::workflow::review::result_schema(),"entry":"implement","nodes":{
            "implement":implement_node(),
            "review":{"type":"review","subject":{"type":"plan","value":{"type":"reference","path":["args"]}},"context":[]},
            "done":{"type":"return","output":{"type":"reference","path":["review"]}}
        },"edges":[{"from":"implement","to":"review"},{"from":"review","to":"done"}]}}))
    .unwrap();
    let program = Arc::new(compile_test(definition).unwrap());
    let runtime = workflow_runtime(&plane);
    let (_, cancellation) = workflow_cancellation();
    let task = tokio::spawn(async move {
        runtime
            .run_test_foreground(
                program,
                ToolCallId::new("post-write"),
                context,
                json!({"changed":IMPLEMENT_FILE}),
                cancellation,
            )
            .await
    });
    child.expect_delegate().await;
    std::fs::write(workspace.join(IMPLEMENT_FILE), "completed effect").unwrap();
    child
        .send_result(
            crate::runtime::subagent::ipc::ChildResultStatus::Succeeded,
            Some("{}"),
        )
        .await;
    let request = published.recv().await.unwrap();
    owner
        .respond_async(&request.id, answer(&request, false))
        .await
        .unwrap();
    assert_eq!(
        task.await.unwrap().unwrap(),
        json!({"accepted":false,"feedback":"Revise scope"})
    );
    assert_eq!(
        std::fs::read_to_string(workspace.join(IMPLEMENT_FILE)).unwrap(),
        "completed effect",
        "Review acquires no filesystem lease and rejection is not rollback"
    );
}

/// C10/C18: a pending Review takes no filesystem lease, so an independent
/// Parallel branch's native Bash write runs and settles while the human
/// decision is still outstanding.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pending_review_does_not_block_an_independent_parallel_branch() {
    let plane = workflow_test_plane(1);
    let workspace = plane.dir.path().join("workspace");
    let (owner, _audit, mut published) = owner(&plane);
    let context = bash_context(&plane, Some(owner.clone()));
    let review_output = crate::runtime::workflow::review::result_schema();
    let definition = serde_json::from_value(json!({"description":"parallel review","timeout_ms":600_000,"block":{
        "input":empty_object(),
        "output":schema(json!({"decision":review_output,"write":check_report()}), &["decision", "write"]),
        "entry":"fanout","nodes":{
            "fanout":{"type":"parallel","branches":{
                "decision":{"input":{"type":"literal","value":{"summary":"ship"}},"block":{
                    "input":schema(json!({"summary":{"type":"string"}}), &["summary"]),
                    "output":review_output,"entry":"review","nodes":{
                        "review":{"type":"review","subject":{"type":"plan","value":{"type":"reference","path":["args"]}},"context":[]},
                        "done":{"type":"return","output":{"type":"reference","path":["review"]}}},
                    "edges":[{"from":"review","to":"done"}]}},
                "write":{"input":{"type":"literal","value":{}},"block":{
                    "input":empty_object(),"output":check_report(),"entry":"write","nodes":{
                        "write":{"type":"tool","selector":{"origin":"builtin","name":"bash"},
                            "arguments":{"type":"literal","value":{"command":format!("printf independent > {IMPLEMENT_FILE}")}},
                            "result":{"type":"json","part":0,"schema":check_report()}},
                        "done":{"type":"return","output":{"type":"reference","path":["write"]}}},
                    "edges":[{"from":"write","to":"done"}]}}}},
            "done":{"type":"return","output":{"type":"reference","path":["fanout"]}}
        },"edges":[{"from":"fanout","to":"done"}]}}))
    .unwrap();
    let program = Arc::new(compile_test(definition).unwrap());
    let runtime = workflow_runtime(&plane);
    let mut observations = runtime.observations.subscribe();
    let (_, cancellation) = workflow_cancellation();
    let task = tokio::spawn(async move {
        runtime
            .run_test_foreground(
                program,
                ToolCallId::new("parallel-review"),
                context,
                json!({}),
                cancellation,
            )
            .await
    });
    let request = published.recv().await.unwrap();
    // The native owner's settlement publication, not elapsed time, proves the
    // independent write branch settled while the Review is still pending.
    observations
        .wait_for(|events| {
            events.iter().any(|event| {
                matches!(event, RuntimeEvent::WorkflowNodeSettled { instance, .. }
                    if instance.node == "write")
            })
        })
        .await
        .unwrap();
    assert!(!owner.pending_snapshot().is_empty());
    assert_eq!(
        std::fs::read_to_string(workspace.join(IMPLEMENT_FILE)).unwrap(),
        "independent"
    );
    owner
        .respond_async(&request.id, answer(&request, true))
        .await
        .unwrap();
    let output = task.await.unwrap().unwrap();
    assert_eq!(output["decision"]["accepted"], true);
    assert_eq!(output["write"]["exit_code"], 0);
}

/// C02/C13: the retired Workflow `workspace` key is refused, and the Runtime
/// Client projections carry no Candidate, handoff or isolation vocabulary.
#[test]
fn retired_workspace_authoring_and_projection_fields_are_gone() {
    let mut definition = serde_json::to_value(super::program_definition()).unwrap();
    assert!(serde_json::from_value::<WorkflowDefinition>(definition.clone()).is_ok());
    definition["workspace"] = json!({"require_clean_parent": true});
    assert!(serde_json::from_value::<WorkflowDefinition>(definition).is_err());
    for schema in [
        serde_json::to_string(&schemars::schema_for!(
            crate::runtime::workflow::read_model::WorkflowRunView
        ))
        .unwrap(),
        serde_json::to_string(&schemars::schema_for!(
            crate::runtime_client::snapshot::RuntimeClientAgent
        ))
        .unwrap(),
    ] {
        for retired in [
            "candidate",
            "handoff",
            "isolation",
            "resource_state",
            "borrowed_from",
        ] {
            assert!(!schema.contains(retired), "{retired} remains in {schema}");
        }
    }
}
