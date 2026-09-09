//! WF-03 cross-owner regressions using the real registry/process settlement.
use super::*;

fn git(root: &std::path::Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .env("GIT_AUTHOR_NAME", "fixture")
        .env("GIT_AUTHOR_EMAIL", "fixture@example.invalid")
        .env("GIT_COMMITTER_NAME", "fixture")
        .env("GIT_COMMITTER_EMAIL", "fixture@example.invalid")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}
fn initialize(plane: &WorkflowTestPlane) {
    let root = plane.dir.path().join("workspace");
    git(&root, &["init"]);
    std::fs::write(root.join("baseline"), b"parent\n").unwrap();
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "baseline"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(clippy::too_many_lines)] // One physical process gate reused for both repair iterations.
async fn loop_checker_supervised_process_gate_blocks_next_writer_and_exhaustion_retains_candidate()
{
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let plane = workflow_test_plane(1);
    initialize(&plane);
    let parent = plane.dir.path().join("workspace");
    std::fs::write(parent.join(".gitignore"), "target/\n").unwrap();
    git(&parent, &["add", ".gitignore"]);
    git(&parent, &["commit", "-m", "ignore checker build output"]);
    let gate = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut first = stage_workflow_child(&plane);
    let mut second = stage_workflow_child(&plane);
    let context = context_with_workspace_policy(
        &plane,
        crate::tools::native::test_bash_registration(),
        crate::agent::AttemptLifecycle::default(),
        WorkspacePolicy::GitWorktree {
            require_clean_parent: true,
        },
    );
    // The shell can emit a checker report and exit while its supervised child
    // still owns candidate work. Only the native Bash supervisor can settle it.
    let command = format!(
        "printf 'provisional checker report\\n'; (exec 3<>/dev/tcp/127.0.0.1/{}; printf 'entered\\n' >&3; IFS= read -r gate <&3; printf 'settled\\n' >> target/check-log) >/dev/null 2>&1 &",
        gate.local_addr().unwrap().port()
    );
    let state = schema(json!({"passed":{"type":"boolean"}}), &["passed"]);
    let definition = serde_json::from_value(json!({"description":"native gated feedback","workspace":{"require_clean_parent":true},"tools":[{"origin":"builtin","name":"bash"}],"block":{
        "input":state,"output":state,"entry":"repair","nodes":{
            "repair":repair_agent(),
            "check":{"type":"tool","selector":{"origin":"builtin","name":"bash"},"arguments":{"type":"literal","value":{"command":command}},"result":{"type":"json","part":0,"schema":{"type":"object"}}},
            "done":{"type":"return","output":{"type":"literal","value":{"passed":false}}}
        },"edges":[{"from":"repair","to":"check"},{"from":"check","to":"done"}]}})).unwrap();
    let program = Arc::new(compile_test(super::loops::wrap_definition(definition, 2)).unwrap());
    let runtime = workflow_runtime(&plane);
    let observations = runtime.observations.subscribe();
    let (_, cancellation) = workflow_cancellation();
    let mut task = tokio::spawn(async move {
        runtime
            .run_foreground(
                program,
                ToolCallId::new("process-loop"),
                context,
                json!({"passed":false}),
                cancellation,
            )
            .await
    });
    let mut candidate_path = None;
    for (index, child) in [(1, &mut first), (2, &mut second)] {
        tokio::select! {
            () = child.expect_delegate() => {},
            result = &mut task => panic!("run stopped before writer: {result:?}"),
        }
        let snapshot = plane.registry.all_snapshots().pop().unwrap();
        let path = snapshot.workspace.logical_workspace;
        if let Some(previous) = &candidate_path {
            assert_eq!(previous, &path);
        }
        candidate_path = Some(path.clone());
        std::fs::create_dir_all(path.join("target")).unwrap();
        if index == 1 {
            std::fs::write(path.join("target/check-log"), "").unwrap();
        }
        std::fs::write(path.join("candidate"), format!("repair-{index}")).unwrap();
        child
            .send_result(
                crate::runtime::subagent::ipc::ChildResultStatus::Succeeded,
                Some("{}"),
            )
            .await;
        let (mut connection, _) = tokio::select! {
            connection = gate.accept() => connection.unwrap(),
            result = &mut task => panic!("run stopped before checker gate: {result:?}"),
        };
        let mut announcement = [0; 8];
        connection.read_exact(&mut announcement).await.unwrap();
        assert_eq!(&announcement, b"entered\n");
        assert_eq!(plane.registry.all_snapshots().len(), index);
        assert_eq!(
            observations
                .borrow()
                .iter()
                .filter(|e| matches!(e, RuntimeEvent::WorkflowLoopIterationSettled { .. }))
                .count(),
            index - 1
        );
        assert!(!task.is_finished());
        connection.write_all(b"release\n").await.unwrap();
    }
    let output = task.await.unwrap().unwrap();
    assert_eq!(output["output"]["status"], "exhausted");
    assert_eq!(output["output"]["iterations"], 2);
    let path = candidate_path.unwrap();
    assert_eq!(
        std::fs::read_to_string(path.join("candidate")).unwrap(),
        "repair-2"
    );
    assert_eq!(
        std::fs::read_to_string(path.join("target/check-log")).unwrap(),
        "settled\nsettled\n"
    );
    assert!(!parent.join("candidate").exists());
    assert!(plane.registry.unsettled_snapshot().is_empty());
    assert!(output.to_string().contains("handoff"));
}
fn candidate_program(agent: bool) -> Arc<WorkflowProgram> {
    Arc::new(compile_test(candidate_definition(agent)).unwrap())
}
fn candidate_definition(agent: bool) -> WorkflowDefinition {
    let mut definition = program_definition();
    definition.workspace = Some(WorkflowWorkspace {
        require_clean_parent: true,
    });
    definition.timeout_ms = 600_000;
    if agent {
        definition.block.nodes.insert(
            "implement".into(),
            WorkflowNodeDefinition::Agent {
                profile: profile("reviewer"),
                task: "Implement the explicitly bound request".into(),
                input: BTreeMap::new(),
                output: schema(json!({}), &[]),
            },
        );
        definition.block.entry = "implement".into();
        definition.block.edges.push(edge("implement", "check"));
    }
    definition
}

#[tokio::test]
async fn loop_failed_and_cancelled_owned_work_preserves_dirty_handoff_without_retry() {
    for mode in ["cancel", "agent_failure", "tool_failure"] {
        let plane = workflow_test_plane(1);
        initialize(&plane);
        let mut child = stage_workflow_child(&plane);
        let probe = Arc::new(CandidateProbe {
            status: if mode == "tool_failure" {
                ToolExecutionStatus::Failed {
                    error: "checker unavailable".into(),
                }
            } else {
                ToolExecutionStatus::Success
            },
            mutate: false,
            observed: std::sync::Mutex::new(Vec::new()),
        });
        let context = setup_context(&plane, probe);
        let runtime = workflow_runtime(&plane);
        let observations = runtime.observations.subscribe();
        let program = Arc::new(
            compile_test(super::loops::wrap_definition(candidate_definition(true), 3)).unwrap(),
        );
        let (trigger, cancellation) = workflow_cancellation();
        let task = tokio::spawn(async move {
            runtime
                .run_foreground(
                    program,
                    ToolCallId::new("failed-loop"),
                    context,
                    json!({"passed":false}),
                    cancellation,
                )
                .await
        });
        child.expect_delegate().await;
        let path = plane.registry.all_snapshots()[0]
            .workspace
            .logical_workspace
            .clone();
        std::fs::write(path.join("candidate"), "useful dirty work").unwrap();
        if mode == "cancel" {
            trigger.cancel();
            child.cancel_after_delegate().await;
        } else {
            child
                .send_result(
                    if mode == "agent_failure" {
                        crate::runtime::subagent::ipc::ChildResultStatus::Failed
                    } else {
                        crate::runtime::subagent::ipc::ChildResultStatus::Succeeded
                    },
                    Some("{}"),
                )
                .await;
        }
        let error = task.await.unwrap().unwrap_err();
        let WorkflowRunError::WorkspaceSettlement { workspace, .. } = &error else {
            panic!("retained handoff: {error:?}")
        };
        assert!(workspace.handoff().is_some());
        assert!(workspace.unresolved_reason().is_none());
        if mode == "cancel" {
            assert!(error.is_cancelled());
        }
        assert_eq!(
            std::fs::read_to_string(path.join("candidate")).unwrap(),
            "useful dirty work"
        );
        assert_eq!(plane.registry.all_snapshots().len(), 1);
        assert!(plane.registry.unsettled_snapshot().is_empty());
        assert_eq!(
            observations
                .borrow()
                .iter()
                .filter(|e| matches!(e, RuntimeEvent::WorkflowLoopIterationAdmitted { .. }))
                .count(),
            1
        );
        assert!(
            !observations
                .borrow()
                .iter()
                .any(|e| matches!(e, RuntimeEvent::WorkflowLoopExited { .. }))
        );
    }
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // Exact two-iteration native candidate and Review ownership sequence.
async fn loop_candidate_mutation_clears_old_acceptance_and_stale_review_cannot_certify_new_version()
{
    use crate::runtime::interaction::InteractionKind;
    let plane = workflow_test_plane(1);
    initialize(&plane);
    let mut first = stage_workflow_child(&plane);
    let mut second = stage_workflow_child(&plane);
    let probe = Arc::new(CandidateProbe {
        status: ToolExecutionStatus::Success,
        mutate: false,
        observed: std::sync::Mutex::new(Vec::new()),
    });
    let (owner, _, mut published) = super::human::owner(&plane);
    let mut context = setup_context(&plane, probe.clone());
    Arc::make_mut(context.native.as_mut().unwrap()).lifecycle =
        crate::agent::AttemptLifecycle::default().with_native_interaction(owner.clone());
    let mut definition = program_definition();
    definition.workspace = Some(WorkflowWorkspace {
        require_clean_parent: true,
    });
    definition.block = serde_json::from_value(json!({
        "input":{"type":"object"},"output":crate::runtime::workflow::review::result_schema(),"entry":"repair",
        "nodes":{"repair":repair_agent(),"review":{"type":"review","subject":{"type":"candidate","value":{"type":"reference","path":["repair"]}},"context":[]},"done":{"type":"return","output":{"type":"reference","path":["review"]}}},
        "edges":[{"from":"repair","to":"review"},{"from":"review","to":"done"}]})).unwrap();
    let mut definition = super::loops::wrap_definition(definition, 2);
    let WorkflowNodeDefinition::Loop { until, carry, .. } =
        definition.block.nodes.get_mut("feedback").unwrap()
    else {
        unreachable!()
    };
    **until = WorkflowPredicate::Boolean {
        value: WorkflowValue::Literal {
            value: json!(false),
        },
    };
    *carry = WorkflowValue::Literal { value: json!({}) };
    let output_schema = definition.block.output.clone();
    definition.block.nodes.insert(
        "check".into(),
        program_definition().block.nodes["check"].clone(),
    );
    // A literal avoids introducing a candidate dependency through old check data.
    let WorkflowNodeDefinition::Tool { arguments, .. } =
        definition.block.nodes.get_mut("check").unwrap()
    else {
        unreachable!()
    };
    *arguments = WorkflowValue::Literal {
        value: json!({"passed":true,"label":"current"}),
    };
    definition.block.edges = vec![
        branch_edge("feedback", "check", WorkflowPort::Satisfied),
        branch_edge("feedback", "check", WorkflowPort::Exhausted),
        edge("check", "done"),
    ];
    definition.block.output = output_schema;
    let runtime = workflow_runtime(&plane);
    let (_, cancellation) = workflow_cancellation();
    let task = tokio::spawn(async move {
        runtime
            .run_foreground(
                Arc::new(compile_test(definition).unwrap()),
                ToolCallId::new("candidate-review-loop"),
                context,
                json!({}),
                cancellation,
            )
            .await
    });
    first.expect_delegate().await;
    let path = plane.registry.all_snapshots()[0]
        .workspace
        .logical_workspace
        .clone();
    std::fs::write(path.join("candidate"), "A").unwrap();
    first
        .send_result(
            crate::runtime::subagent::ipc::ChildResultStatus::Succeeded,
            Some("{}"),
        )
        .await;
    let a = published.recv().await.unwrap();
    owner
        .respond_async(&a.id, super::human::answer(&a, true))
        .await
        .unwrap();
    second.expect_delegate().await;
    std::fs::write(path.join("candidate"), "B").unwrap();
    second
        .send_result(
            crate::runtime::subagent::ipc::ChildResultStatus::Succeeded,
            Some("{}"),
        )
        .await;
    let b = published.recv().await.unwrap();
    let (InteractionKind::Review { review: ar, .. }, InteractionKind::Review { review: br, .. }) =
        (&a.kind, &b.kind)
    else {
        panic!("reviews")
    };
    assert_ne!(ar.candidate().unwrap(), br.candidate().unwrap());
    assert_ne!(ar.instance, br.instance);
    assert!(
        owner
            .respond_async(&b.id, super::human::answer(&a, true))
            .await
            .is_err()
    );
    assert!(
        owner
            .respond_async(&a.id, super::human::answer(&a, true))
            .await
            .is_err()
    );
    owner
        .respond_async(&b.id, super::human::answer(&b, false))
        .await
        .unwrap();
    let output = task.await.unwrap().unwrap();
    assert_eq!(output["output"]["status"], "exhausted");
    assert_eq!(output["output"]["result"]["accepted"], false);
    assert_eq!(
        probe.observed.lock().unwrap().len(),
        1,
        "current B can be checked after old A acceptance was cleared"
    );
    assert_eq!(
        std::fs::read_to_string(path.join("candidate")).unwrap(),
        "B"
    );
    assert_eq!(owner.pending_count(), 0);
    assert!(plane.registry.unsettled_snapshot().is_empty());
}

struct CandidateProbe {
    status: ToolExecutionStatus,
    mutate: bool,
    observed: std::sync::Mutex<Vec<(std::path::PathBuf, Vec<u8>)>>,
}
impl ToolExecutor for CandidateProbe {
    fn workspace_use(&self) -> crate::tools::executor::WorkspaceUse {
        crate::tools::executor::WorkspaceUse::ConsumesProvided
    }
    fn progress_capability(&self) -> crate::tools::deadline::ToolProgressCapability {
        crate::tools::deadline::ToolProgressCapability::None
    }
    fn start<'a>(
        &'a self,
        _: ToolInvocation,
        context: ToolExecutionContext<'a>,
    ) -> ToolExecutionHandle<'a> {
        let cancellation = context.cancellation.clone();
        ToolExecutionHandle::settled_by_operation(
            Box::pin(async move {
                let root = context.workspace.root();
                self.observed.lock().unwrap().push((
                    root.to_path_buf(),
                    std::fs::read(root.join("candidate")).unwrap_or_default(),
                ));
                if self.mutate {
                    std::fs::write(root.join("candidate"), b"validator changed source").unwrap();
                }
                let mut result = terminal(self.status.clone());
                result.content = vec![
                    ToolResultContent::Text(crate::message::content::TextBlock {
                        text: "actual invocation".into(),
                    }),
                    ToolResultContent::Json {
                        value: json!({"passed":true}),
                    },
                ];
                result
            }),
            cancellation,
        )
    }
}

fn setup_context(
    plane: &WorkflowTestPlane,
    probe: Arc<CandidateProbe>,
) -> crate::runtime::subagent::AttemptSubagentContext {
    context_with_workspace_policy(
        plane,
        ToolRegistration::plain(definition(), probe),
        crate::agent::AttemptLifecycle::default(),
        WorkspacePolicy::GitWorktree {
            require_clean_parent: true,
        },
    )
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // One cross-owner fixture checks bytes, authority and terminal ordering.
async fn agent_dirty_bytes_reach_exact_tool_context_after_child_settlement_with_frozen_resources() {
    let plane = workflow_test_plane(1);
    initialize(&plane);
    let mut child = stage_workflow_child(&plane);
    let probe = Arc::new(CandidateProbe {
        status: ToolExecutionStatus::Success,
        mutate: false,
        observed: std::sync::Mutex::default(),
    });
    let context = setup_context(&plane, probe.clone());
    let resources = context.resources().clone();
    let frozen_skills = resources.skill_catalog().map(str::to_owned);
    let frozen_instructions = resources.project_instructions().map(str::to_owned);
    let frozen = context.resolve_workflow(&profile("reviewer")).unwrap();
    let runtime = workflow_runtime(&plane);
    let (_, cancellation) = workflow_cancellation();
    let task = tokio::spawn(async move {
        runtime
            .run_foreground(
                candidate_program(true),
                ToolCallId::new("candidate-run"),
                context,
                json!({"passed":true}),
                cancellation,
            )
            .await
    });
    child.expect_delegate().await;
    let snapshot = plane.registry.all_snapshots().pop().unwrap();
    let path = snapshot.workspace.logical_workspace.clone();
    assert!(snapshot.workspace.borrowed_from.is_some());
    std::fs::write(
        path.join("candidate"),
        b"uncommitted implementation bytes\0",
    )
    .unwrap();
    let parent = plane.dir.path().join("workspace");
    std::fs::write(parent.join("AGENTS.md"), b"replacement instructions").unwrap();
    std::fs::create_dir_all(parent.join(".agents/skills/new")).unwrap();
    std::fs::write(parent.join(".agents/skills/new/SKILL.md"), b"new skill").unwrap();
    std::fs::write(
        parent.join("rustx.jsonc"),
        b"replacement tool and profile definitions",
    )
    .unwrap();
    git(&parent, &["add", "."]);
    git(&parent, &["commit", "-m", "parent advanced"]);
    child
        .send_result(
            crate::runtime::subagent::ipc::ChildResultStatus::Succeeded,
            Some("{}"),
        )
        .await;
    let result = task.await.unwrap().unwrap();
    assert_eq!(result["output"], json!({"passed":true}));
    assert_eq!(
        probe.observed.lock().unwrap().as_slice(),
        &[(
            path.canonicalize().unwrap(),
            b"uncommitted implementation bytes\0".to_vec()
        )]
    );
    assert!(path.exists());
    assert!(!parent.join("candidate").exists());
    let settled = plane.registry.all_snapshots().pop().unwrap();
    assert!(settled.settled);
    assert!(settled.handoff.is_none());
    assert_eq!(
        settled.workspace_resource_state,
        crate::runtime::subagent::SubagentWorkspaceResourceState::None
    );
    assert_eq!(resources.revision().get(), 1);
    assert_eq!(resources.skill_catalog(), frozen_skills.as_deref());
    assert_eq!(
        resources.project_instructions(),
        frozen_instructions.as_deref()
    );
    assert_eq!(
        resources
            .subagents()
            .get(&profile("reviewer"))
            .unwrap()
            .instructions(),
        frozen.instructions
    );
    assert!(
        resources
            .capability()
            .tool_registry()
            .model_definitions()
            .is_empty()
    );
    let events = plane.store.read_events(None, 256).unwrap().events;
    assert!(events.iter().any(|event| matches!(
        &event.event,
        RuntimeEvent::WorkflowCandidateInvocation {
            candidate_unchanged: true,
            result: ToolExecutionStatus::Success,
            ..
        }
    )));
    assert!(matches!(
        events.last().unwrap().event,
        RuntimeEvent::WorkflowCompleted { .. }
    ));
}

#[tokio::test]
async fn candidate_profile_conflict_and_unsupported_executor_fail_before_git_or_child_side_effects()
{
    for unsupported in [false, true] {
        let plane = workflow_test_plane(1);
        let probe = Probe::new(ToolExecutionStatus::Success);
        let context = context(
            &plane,
            probe.clone(),
            crate::agent::AttemptLifecycle::default(),
        );
        let (_, cancellation) = workflow_cancellation();
        let error = workflow_runtime(&plane)
            .run_foreground(
                candidate_program(!unsupported),
                ToolCallId::new("rejected"),
                context,
                json!({"passed":true}),
                cancellation,
            )
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            WorkflowRunError::InvalidProgram(_) | WorkflowRunError::IneligibleCapability(_)
        ));
        assert_eq!(probe.starts.load(Ordering::SeqCst), 0);
        assert!(plane.registry.all_snapshots().is_empty());
        assert!(!plane.runtime_root.join("worktrees").exists());
    }
}

#[tokio::test]
async fn tool_mutation_invalidates_actual_invocation_and_retains_failed_candidate() {
    let plane = workflow_test_plane(1);
    initialize(&plane);
    let probe = Arc::new(CandidateProbe {
        status: ToolExecutionStatus::Success,
        mutate: true,
        observed: std::sync::Mutex::default(),
    });
    let context = setup_context(&plane, probe);
    let (_, cancellation) = workflow_cancellation();
    let error = workflow_runtime(&plane)
        .run_foreground(
            candidate_program(false),
            ToolCallId::new("mutating-check"),
            context,
            json!({"passed":true}),
            cancellation,
        )
        .await
        .unwrap_err();
    let WorkflowRunError::WorkspaceSettlement {
        workspace, error, ..
    } = error
    else {
        panic!("missing retained failure facts")
    };
    assert!(workspace.handoff().unwrap().dirty);
    assert!(matches!(
        error.execution_status(),
        ToolExecutionStatus::Failed { .. }
    ));
    assert_eq!(
        std::fs::read(workspace.snapshot.logical_workspace.join("candidate")).unwrap(),
        b"validator changed source"
    );
    let events = plane.store.read_events(None, 256).unwrap().events;
    assert!(events.iter().any(|event| matches!(
        event.event,
        RuntimeEvent::WorkflowCandidateInvocation {
            candidate_unchanged: false,
            ..
        }
    )));
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // Exercises the four terminal classes against one physical child fixture.
async fn cancellation_agent_failure_and_tool_failure_share_dirty_run_handoff() {
    for failure in 0..4 {
        let plane = workflow_test_plane(1);
        initialize(&plane);
        let mut child = stage_workflow_child(&plane);
        let probe = Arc::new(CandidateProbe {
            status: if failure == 3 {
                ToolExecutionStatus::Success
            } else {
                ToolExecutionStatus::Failed {
                    error: "native check failed".into(),
                }
            },
            mutate: false,
            observed: std::sync::Mutex::default(),
        });
        let context = setup_context(&plane, probe);
        let runtime = workflow_runtime(&plane);
        let (trigger, cancellation) = workflow_cancellation();
        let program = if failure == 3 {
            // Native Tool succeeds, but its fixed projection is invalid: a
            // generic block failure must use the same dirty-resource path.
            let mut definition = program_definition();
            definition.workspace = Some(WorkflowWorkspace {
                require_clean_parent: true,
            });
            definition.block.nodes.insert(
                "implement".into(),
                WorkflowNodeDefinition::Agent {
                    profile: profile("reviewer"),
                    task: "write".into(),
                    input: BTreeMap::new(),
                    output: schema(json!({}), &[]),
                },
            );
            definition.block.entry = "implement".into();
            definition.block.edges.push(edge("implement", "check"));
            if let WorkflowNodeDefinition::Tool {
                result: WorkflowToolResult::Json { part, .. },
                ..
            } = definition.block.nodes.get_mut("check").unwrap()
            {
                *part = 99;
            }
            Arc::new(compile_test(definition).unwrap())
        } else {
            candidate_program(true)
        };
        let task = tokio::spawn(async move {
            runtime
                .run_foreground(
                    program,
                    ToolCallId::new("failed-run"),
                    context,
                    json!({"passed":true}),
                    cancellation,
                )
                .await
        });
        child.expect_delegate().await;
        let path = plane.registry.all_snapshots()[0]
            .workspace
            .logical_workspace
            .clone();
        std::fs::write(path.join("candidate"), b"useful failed work").unwrap();
        if failure == 0 {
            trigger.cancel();
            child.cancel_after_delegate().await;
        } else {
            child
                .send_result(
                    if failure == 1 {
                        crate::runtime::subagent::ipc::ChildResultStatus::Failed
                    } else {
                        crate::runtime::subagent::ipc::ChildResultStatus::Succeeded
                    },
                    (failure >= 2).then_some("{}"),
                )
                .await;
        }
        let WorkflowRunError::WorkspaceSettlement { workspace, .. } =
            task.await.unwrap().unwrap_err()
        else {
            panic!("failure lost workspace facts")
        };
        assert_eq!(workspace.handoff().unwrap().logical_workspace, path);
        assert_eq!(
            std::fs::read(path.join("candidate")).unwrap(),
            b"useful failed work"
        );
        assert!(plane.registry.unsettled_snapshot().is_empty());
        let events = plane.store.read_events(None, 256).unwrap().events;
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(
                    event.event,
                    RuntimeEvent::WorkflowWorkspaceSettled { .. }
                ))
                .count(),
            1
        );
    }
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // Checks identity rejection, physical disposal and immutable terminal facts together.
async fn retained_run_disposal_is_identity_only_idempotent_and_preserves_terminal_outcome() {
    let plane = workflow_test_plane(1);
    initialize(&plane);
    let probe = Arc::new(CandidateProbe {
        status: ToolExecutionStatus::Success,
        mutate: true,
        observed: std::sync::Mutex::default(),
    });
    let context = setup_context(&plane, probe);
    let (_, cancellation) = workflow_cancellation();
    let error = workflow_runtime(&plane)
        .run_foreground(
            candidate_program(false),
            ToolCallId::new("retained"),
            context,
            json!({"passed":true}),
            cancellation,
        )
        .await
        .unwrap_err();
    let WorkflowRunError::WorkspaceSettlement { workspace, .. } = error else {
        panic!("retained facts")
    };
    let events = plane.store.read_events(None, 256).unwrap().events;
    let (run, terminal) = events
        .iter()
        .find_map(|event| match &event.event {
            RuntimeEvent::WorkflowWorkspaceSettled { run_id, .. } => {
                Some((run_id.clone(), event.clone()))
            }
            _ => None,
        })
        .unwrap();
    let manager = WorkspaceManager::new(plane.dir.path().join("workspace"), &plane.runtime_root);
    let mut missing = run.clone();
    missing.invocation += 1;
    assert!(
        manager
            .dispose_workflow_workspace(&*plane.store, &missing)
            .await
            .is_err()
    );
    let unrelated = tempfile::tempdir().unwrap();
    let wrong = WorkspaceManager::new(plane.dir.path().join("workspace"), unrelated.path());
    assert!(
        wrong
            .dispose_workflow_workspace(&*plane.store, &run)
            .await
            .is_err()
    );
    assert_eq!(
        WorkspaceManager::inspect_workflow_workspace(&*plane.store, &run)
            .unwrap()
            .settlement,
        *workspace
    );
    let source = workspace.snapshot.logical_workspace.join("candidate");
    let retained_bytes = std::fs::read(&source).unwrap();
    std::fs::write(&source, b"later external work at the same HEAD").unwrap();
    assert!(
        manager
            .dispose_workflow_workspace(&*plane.store, &run)
            .await
            .is_err()
    );
    assert_eq!(
        std::fs::read(&source).unwrap(),
        b"later external work at the same HEAD"
    );
    std::fs::write(&source, retained_bytes).unwrap();
    assert_eq!(
        manager
            .dispose_workflow_workspace(&*plane.store, &run)
            .await
            .unwrap(),
        crate::runtime::workspace::WorkspaceDisposalSettlement::Disposed
    );
    assert!(!workspace.snapshot.logical_workspace.exists());
    assert!(
        WorkspaceManager::inspect_workflow_workspace(&*plane.store, &run)
            .unwrap()
            .disposed
    );
    assert_eq!(
        manager
            .dispose_workflow_workspace(&*plane.store, &run)
            .await
            .unwrap(),
        crate::runtime::workspace::WorkspaceDisposalSettlement::AlreadyDisposed
    );
    assert!(
        plane.store.append_event(terminal).is_err(),
        "terminal cannot be committed twice"
    );
    let events = plane.store.read_events(None, 256).unwrap().events;
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event.event, RuntimeEvent::WorkflowFailed { .. }))
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(
                event.event,
                RuntimeEvent::WorkflowWorkspaceDisposalStarted { .. }
            ))
            .count(),
        1
    );
    assert_eq!(
        std::fs::read(plane.dir.path().join("workspace/baseline")).unwrap(),
        b"parent\n"
    );
}

#[tokio::test]
async fn reopened_journal_retains_resource_facts_without_recreating_borrowers() {
    let plane = workflow_test_plane(1);
    initialize(&plane);
    let probe = Arc::new(CandidateProbe {
        status: ToolExecutionStatus::Success,
        mutate: true,
        observed: std::sync::Mutex::default(),
    });
    let context = setup_context(&plane, probe);
    let (_, cancellation) = workflow_cancellation();
    let error = workflow_runtime(&plane)
        .run_foreground(
            candidate_program(false),
            ToolCallId::new("restart"),
            context,
            json!({"passed":true}),
            cancellation,
        )
        .await
        .unwrap_err();
    let WorkflowRunError::WorkspaceSettlement { workspace, .. } = error else {
        panic!("retained")
    };
    let events = plane.store.read_events(None, 256).unwrap().events;
    let run = events
        .iter()
        .find_map(|event| match &event.event {
            RuntimeEvent::WorkflowWorkspaceOwned { run_id, .. } => Some(run_id.clone()),
            _ => None,
        })
        .unwrap();
    let database = plane.dir.path().join("reopened.sqlite");
    {
        let store =
            crate::durable::SqliteConversationStore::open(run.conversation_id.clone(), &database)
                .unwrap();
        for mut event in events.into_iter().filter(|event| {
            matches!(
                event.event,
                RuntimeEvent::WorkflowWorkspaceOwned { .. }
                    | RuntimeEvent::WorkflowWorkspaceSettled { .. }
            )
        }) {
            event.sequence = 0;
            store.append_event(event).unwrap();
        }
    }
    let store =
        crate::durable::SqliteConversationStore::open(run.conversation_id.clone(), &database)
            .unwrap();
    let facts = WorkspaceManager::inspect_workflow_workspace(&store, &run).unwrap();
    assert_eq!(facts.settlement, *workspace);
    assert!(!facts.disposed);
    assert!(plane.registry.unsettled_snapshot().is_empty());
    let manager = WorkspaceManager::new(plane.dir.path().join("workspace"), &plane.runtime_root);
    assert_eq!(
        manager
            .dispose_workflow_workspace(&store, &run)
            .await
            .unwrap(),
        crate::runtime::workspace::WorkspaceDisposalSettlement::Disposed
    );
    drop(store);
    let store =
        crate::durable::SqliteConversationStore::open(run.conversation_id.clone(), &database)
            .unwrap();
    let facts = WorkspaceManager::inspect_workflow_workspace(&store, &run).unwrap();
    assert!(facts.disposed);
    assert_eq!(
        facts.settlement, *workspace,
        "historical terminal fact is immutable"
    );
    assert_eq!(
        manager
            .dispose_workflow_workspace(&store, &run)
            .await
            .unwrap(),
        crate::runtime::workspace::WorkspaceDisposalSettlement::AlreadyDisposed
    );
}

struct GatedCandidateProbe {
    started: tokio::sync::watch::Sender<usize>,
    cancelled: tokio::sync::watch::Sender<bool>,
    release: tokio::sync::Semaphore,
}
impl ToolExecutor for GatedCandidateProbe {
    fn workspace_use(&self) -> crate::tools::executor::WorkspaceUse {
        crate::tools::executor::WorkspaceUse::ConsumesProvided
    }
    fn progress_capability(&self) -> crate::tools::deadline::ToolProgressCapability {
        crate::tools::deadline::ToolProgressCapability::None
    }
    fn start<'a>(
        &'a self,
        _: ToolInvocation,
        context: ToolExecutionContext<'a>,
    ) -> ToolExecutionHandle<'a> {
        let cancellation = context.cancellation.clone();
        ToolExecutionHandle::settled_by_operation(
            Box::pin(async move {
                self.started.send_modify(|count| *count += 1);
                tokio::select! {
                    biased;
                    () = context.cancellation.cancelled() => {
                        self.cancelled.send_replace(true);
                        self.release.acquire().await.unwrap().forget();
                        terminal(ToolExecutionStatus::Cancelled { reason: context.cancellation.reason(), phase: ToolCancellationPhase::DuringExecution })
                    }
                    permit = self.release.acquire() => {
                        permit.unwrap().forget();
                        let mut result = terminal(ToolExecutionStatus::Success);
                        result.content = vec![ToolResultContent::Text(crate::message::content::TextBlock { text: "settled".into() }), ToolResultContent::Json { value: json!({"passed":true}) }]; result
                    }
                }
            }),
            cancellation,
        )
    }
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // Two gated terminal paths share the exact same concurrency fixture.
async fn parallel_candidate_consumers_serialize_and_cancellation_waits_for_physical_tool_settlement()
 {
    for cancel in [false, true] {
        let plane = workflow_test_plane(1);
        initialize(&plane);
        let probe = Arc::new(GatedCandidateProbe {
            started: tokio::sync::watch::Sender::new(0),
            cancelled: tokio::sync::watch::Sender::new(false),
            release: tokio::sync::Semaphore::new(0),
        });
        let mut tool = definition();
        tool.concurrency_policy = crate::tools::types::ToolConcurrencyPolicy::Parallel;
        let context = context_with_workspace_policy(
            &plane,
            ToolRegistration::plain(tool, probe.clone()),
            crate::agent::AttemptLifecycle::default(),
            WorkspacePolicy::GitWorktree {
                require_clean_parent: true,
            },
        );
        let mut definition = program_definition();
        definition.workspace = Some(WorkflowWorkspace {
            require_clean_parent: true,
        });
        definition.timeout_ms = 600_000;
        let branch = WorkflowBranch {
            input: WorkflowValue::Reference {
                path: vec!["args".into()],
            },
            block: definition.block.clone(),
        };
        definition.block = WorkflowBlock {
            input: branch.block.input.clone(),
            output: schema(
                json!({"a":branch.block.output.clone(),"b":branch.block.output.clone()}),
                &["a", "b"],
            ),
            entry: "parallel".into(),
            nodes: BTreeMap::from([
                (
                    "parallel".into(),
                    WorkflowNodeDefinition::Parallel {
                        branches: BTreeMap::from([
                            ("a".into(), branch.clone()),
                            ("b".into(), branch),
                        ]),
                    },
                ),
                (
                    "done".into(),
                    WorkflowNodeDefinition::Return {
                        output: WorkflowValue::Reference {
                            path: vec!["parallel".into()],
                        },
                    },
                ),
            ]),
            edges: vec![edge("parallel", "done")],
        };
        let program = Arc::new(compile_test(definition).unwrap());
        let runtime = workflow_runtime(&plane);
        let mut observations = runtime.observations.subscribe();
        let mut started = probe.started.subscribe();
        let (trigger, cancellation) = workflow_cancellation();
        let task = tokio::spawn(async move {
            runtime
                .run_foreground(
                    program,
                    ToolCallId::new("parallel-candidates"),
                    context,
                    json!({"passed":true}),
                    cancellation,
                )
                .await
        });
        started.wait_for(|count| *count == 1).await.unwrap();
        observations.wait_for(|events| events.iter().filter(|event| matches!(event, RuntimeEvent::WorkflowNodeStarted { instance } if instance.node == "check")).count() == 2).await.unwrap();
        assert_eq!(
            *started.borrow(),
            1,
            "second node reached admission but has no physical access"
        );
        if cancel {
            let mut cancelled = probe.cancelled.subscribe();
            trigger.cancel();
            cancelled.wait_for(|seen| *seen).await.unwrap();
            assert!(
                !task.is_finished(),
                "cancellation intent is not physical settlement"
            );
            assert!(
                !plane
                    .store
                    .read_events(None, 256)
                    .unwrap()
                    .events
                    .iter()
                    .any(|event| matches!(
                        event.event,
                        RuntimeEvent::WorkflowWorkspaceSettled { .. }
                    ))
            );
            probe.release.add_permits(1);
            assert!(task.await.unwrap().is_err());
            assert_eq!(*started.borrow(), 1);
        } else {
            probe.release.add_permits(1);
            started.wait_for(|count| *count == 2).await.unwrap();
            probe.release.add_permits(1);
            assert!(task.await.unwrap().is_ok());
        }
    }
}

#[tokio::test]
async fn stale_check_cannot_drive_branch_after_writer_commits_new_candidate() {
    stale_check_after_writer(false, false).await;
}

#[tokio::test]
async fn stale_check_cannot_escape_return_through_object_and_array_construction() {
    stale_check_after_writer(true, false).await;
}

#[tokio::test]
async fn parallel_export_preserves_check_applicability_across_later_writer() {
    stale_check_after_writer(false, true).await;
}

#[allow(clippy::too_many_lines)] // One composed sequence with distinct consumption frontiers.
async fn stale_check_after_writer(return_value: bool, parallel_export: bool) {
    let plane = workflow_test_plane(1);
    initialize(&plane);
    let mut child = stage_workflow_child(&plane);
    let probe = Arc::new(CandidateProbe {
        status: ToolExecutionStatus::Success,
        mutate: false,
        observed: std::sync::Mutex::default(),
    });
    let context = setup_context(&plane, probe.clone());
    let mut definition = program_definition();
    definition.workspace = Some(WorkflowWorkspace {
        require_clean_parent: true,
    });
    definition.block.nodes.insert(
        "writer".into(),
        WorkflowNodeDefinition::Agent {
            profile: profile("reviewer"),
            task: "write B".into(),
            input: BTreeMap::new(),
            output: schema(json!({}), &[]),
        },
    );
    definition.block.edges.retain(|edge| edge.from != "check");
    definition.block.edges.push(edge("check", "writer"));
    if return_value {
        definition
            .block
            .nodes
            .retain(|key, _| key == "check" || key == "writer");
        definition.block.output = schema(
            json!({"checks":{"type":"array","items":{"type":"boolean"}}}),
            &["checks"],
        );
        definition.block.nodes.insert(
            "return".into(),
            WorkflowNodeDefinition::Return {
                output: WorkflowValue::Object {
                    fields: BTreeMap::from([(
                        "checks".into(),
                        WorkflowValue::Array {
                            items: vec![WorkflowValue::Reference {
                                path: vec!["check".into(), "passed".into()],
                            }],
                        },
                    )]),
                },
            },
        );
        definition.block.edges.retain(|edge| edge.from == "check");
        definition.block.edges.push(edge("writer", "return"));
    } else {
        definition.block.edges.push(edge("writer", "branch"));
    }
    if parallel_export {
        let check_schema = schema(json!({"passed":{"type":"boolean"}}), &["passed"]);
        definition.block.nodes.insert(
            "relay".into(),
            WorkflowNodeDefinition::Parallel {
                branches: BTreeMap::from([(
                    "copy".into(),
                    WorkflowBranch {
                        input: WorkflowValue::Object {
                            fields: BTreeMap::from([(
                                "passed".into(),
                                WorkflowValue::Reference {
                                    path: vec!["check".into(), "passed".into()],
                                },
                            )]),
                        },
                        block: WorkflowBlock {
                            input: check_schema.clone(),
                            output: check_schema,
                            entry: "export".into(),
                            nodes: BTreeMap::from([(
                                "export".into(),
                                WorkflowNodeDefinition::Return {
                                    output: WorkflowValue::Reference {
                                        path: vec!["args".into()],
                                    },
                                },
                            )]),
                            edges: vec![],
                        },
                    },
                )]),
            },
        );
        definition.block.edges.retain(|edge| edge.from != "check");
        definition.block.edges.push(edge("check", "relay"));
        definition.block.edges.push(edge("relay", "writer"));
        definition.block.nodes.insert(
            "branch".into(),
            WorkflowNodeDefinition::Branch {
                condition: WorkflowPredicate::Boolean {
                    value: WorkflowValue::Reference {
                        path: vec!["relay".into(), "copy".into(), "passed".into()],
                    },
                },
            },
        );
    }
    let program = Arc::new(compile_test(definition).unwrap());
    let runtime = workflow_runtime(&plane);
    let (_, cancellation) = workflow_cancellation();
    let task = tokio::spawn(async move {
        runtime
            .run_foreground(
                program,
                ToolCallId::new("stale-check"),
                context,
                json!({"passed":true}),
                cancellation,
            )
            .await
    });
    child.expect_delegate().await; // check A is already committed
    assert_eq!(probe.observed.lock().unwrap().len(), 1);
    let snapshot = plane.registry.all_snapshots().pop().unwrap();
    std::fs::write(
        snapshot.workspace.logical_workspace.join("candidate"),
        b"writer produced B",
    )
    .unwrap();
    child
        .send_result(
            crate::runtime::subagent::ipc::ChildResultStatus::Succeeded,
            Some("{}"),
        )
        .await;
    let error = task.await.unwrap().unwrap_err();
    let WorkflowRunError::WorkspaceSettlement { error, .. } = error else {
        panic!("missing resource settlement")
    };
    assert!(
        matches!(*error, WorkflowRunError::InvalidValue(ref detail) if detail.contains("stale candidate reference"))
    );
    assert_eq!(probe.observed.lock().unwrap().len(), 1);
    assert_eq!(
        plane.registry.all_snapshots().len(),
        1,
        "exactly one writer"
    );
    assert!(plane.registry.unsettled_snapshot().is_empty());
    let events = plane.store.read_events(None, 256).unwrap().events;
    assert_eq!(events.iter().filter(|event| matches!(&event.event, RuntimeEvent::WorkflowCandidateInvocation { result: ToolExecutionStatus::Success, candidate_unchanged: true, input, .. } if input.version == 0)).count(), 1);
    assert_eq!(events.iter().filter(|event| matches!(&event.event, RuntimeEvent::WorkflowNodeStarted { instance } if instance.node == "yes" || instance.node == "no")).count(), 0);
    assert!(
        !events
            .iter()
            .any(|event| matches!(event.event, RuntimeEvent::WorkflowBranchSelected { .. }))
    );
    for node in [
        "check",
        "writer",
        if return_value { "return" } else { "branch" },
    ] {
        assert_eq!(events.iter().filter(|event| matches!(&event.event, RuntimeEvent::WorkflowNodeStarted { instance } if instance.node == node)).count(), usize::from(node == "check" || node == "writer"), "exact start count for {node}");
    }
    assert!(events.iter().any(|event| matches!(&event.event, RuntimeEvent::WorkflowWorkspaceSettled { candidate: Some(reference), .. } if reference.version == 1)));
}

fn agent_review_program(writer: bool, parallel: bool) -> Arc<WorkflowProgram> {
    let result_schema = schema(json!({"passed":{"type":"boolean"}}), &["passed"]);
    let review = WorkflowNodeDefinition::Agent {
        profile: profile("reviewer"),
        task: "Review the current candidate".into(),
        input: BTreeMap::new(),
        output: result_schema.clone(),
    };
    let mut definition = program_definition();
    definition.workspace = Some(WorkflowWorkspace {
        require_clean_parent: true,
    });
    definition.timeout_ms = 600_000;
    let (entry, producer, review_node) = if parallel {
        (
            "parallel",
            vec!["parallel".into(), "copy".into()],
            WorkflowNodeDefinition::Parallel {
                branches: BTreeMap::from([(
                    "copy".into(),
                    WorkflowBranch {
                        input: WorkflowValue::Literal { value: json!({}) },
                        block: WorkflowBlock {
                            input: schema(json!({}), &[]),
                            output: result_schema.clone(),
                            entry: "review".into(),
                            nodes: BTreeMap::from([
                                ("review".into(), review),
                                (
                                    "export".into(),
                                    WorkflowNodeDefinition::Return {
                                        output: WorkflowValue::Reference {
                                            path: vec!["review".into()],
                                        },
                                    },
                                ),
                            ]),
                            edges: vec![edge("review", "export")],
                        },
                    },
                )]),
            },
        )
    } else {
        ("review", vec!["review".into()], review)
    };
    let mut predicate = producer.clone();
    predicate.push("passed".into());
    definition.block.nodes = BTreeMap::from([
        (entry.into(), review_node),
        (
            "branch".into(),
            WorkflowNodeDefinition::Branch {
                condition: WorkflowPredicate::Boolean {
                    value: WorkflowValue::Reference { path: predicate },
                },
            },
        ),
        (
            "yes".into(),
            WorkflowNodeDefinition::Return {
                output: WorkflowValue::Reference {
                    path: producer.clone(),
                },
            },
        ),
        (
            "no".into(),
            WorkflowNodeDefinition::Return {
                output: WorkflowValue::Reference { path: producer },
            },
        ),
    ]);
    definition.block.entry = entry.into();
    definition.block.edges.retain(|edge| edge.from == "branch");
    if writer {
        definition.block.nodes.insert(
            "writer".into(),
            WorkflowNodeDefinition::Agent {
                profile: profile("reviewer"),
                task: "Produce B".into(),
                input: BTreeMap::new(),
                output: schema(json!({}), &[]),
            },
        );
        definition
            .block
            .edges
            .extend([edge(entry, "writer"), edge("writer", "branch")]);
    } else {
        definition.block.edges.push(edge(entry, "branch"));
    }
    Arc::new(compile_test(definition).unwrap())
}

#[tokio::test]
async fn machine_review_agent_a_cannot_authorize_b_after_writer_settles() {
    agent_review_case(true, false).await;
}

#[tokio::test]
async fn machine_review_agent_a_selects_true_while_candidate_remains_a() {
    agent_review_case(false, false).await;
}

#[tokio::test]
async fn parallel_agent_review_export_remains_bound_to_a_after_writer_b() {
    agent_review_case(true, true).await;
}

#[allow(clippy::too_many_lines)]
async fn agent_review_case(writer: bool, parallel: bool) {
    let plane = workflow_test_plane(1);
    initialize(&plane);
    let mut review = stage_workflow_child(&plane);
    let mut writer_child = writer.then(|| stage_workflow_child(&plane));
    let probe = Arc::new(CandidateProbe {
        status: ToolExecutionStatus::Success,
        mutate: false,
        observed: std::sync::Mutex::default(),
    });
    let context = setup_context(&plane, probe.clone());
    let runtime = workflow_runtime(&plane);
    let program = agent_review_program(writer, parallel);
    let (_, cancellation) = workflow_cancellation();
    let task = tokio::spawn(async move {
        runtime
            .run_foreground(
                program,
                ToolCallId::new("machine-review"),
                context,
                json!({"passed":true}),
                cancellation,
            )
            .await
    });
    review.expect_delegate().await;
    review
        .send_result(
            crate::runtime::subagent::ipc::ChildResultStatus::Succeeded,
            Some(r#"{"passed":true}"#),
        )
        .await;
    let mut retained_path = None;
    if let Some(child) = writer_child.as_mut() {
        child.expect_delegate().await; // review's physical settlement/local commit precedes writer
        let path = plane
            .registry
            .all_snapshots()
            .last()
            .unwrap()
            .workspace
            .logical_workspace
            .clone();
        std::fs::write(path.join("candidate"), b"writer B").unwrap();
        retained_path = Some(path);
        child
            .send_result(
                crate::runtime::subagent::ipc::ChildResultStatus::Succeeded,
                Some("{}"),
            )
            .await;
    }
    let result = task.await.unwrap();
    if writer {
        let WorkflowRunError::WorkspaceSettlement {
            error,
            workspace,
            candidate,
        } = result.unwrap_err()
        else {
            panic!("missing retained settlement")
        };
        assert!(
            matches!(*error, WorkflowRunError::InvalidValue(ref detail) if detail.contains("stale candidate reference"))
        );
        assert_eq!(candidate.unwrap().version, 1);
        assert!(workspace.handoff().unwrap().dirty);
        assert_eq!(
            std::fs::read(retained_path.unwrap().join("candidate")).unwrap(),
            b"writer B"
        );
    } else {
        let result = result.unwrap();
        assert_eq!(result["output"], json!({"passed":true}));
        assert_eq!(result["candidate"]["version"], 0);
    }
    let events = plane.store.read_events(None, 256).unwrap().events;
    let starts = |node: &str| {
        events.iter().filter(|e| matches!(&e.event, RuntimeEvent::WorkflowNodeStarted { instance } if instance.node == node)).count()
    };
    assert_eq!(starts("review"), 1);
    assert_eq!(starts("writer"), usize::from(writer));
    assert_eq!(starts("branch"), usize::from(!writer));
    assert_eq!(starts("yes"), usize::from(!writer));
    assert_eq!(starts("no"), 0);
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(
                &e.event,
                RuntimeEvent::WorkflowBranchSelected {
                    port: WorkflowPort::True,
                    ..
                }
            ))
            .count(),
        usize::from(!writer)
    );
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(&e.event, RuntimeEvent::WorkflowBranchSelected { .. }))
            .count(),
        usize::from(!writer)
    );
    assert_eq!(events.iter().filter(|e| matches!(&e.event, RuntimeEvent::WorkflowAgentOutputCommitted { node_id, output, .. } if node_id.node == "review" && *output == json!({"passed":true}))).count(), 1);
    assert_eq!(
        plane.registry.all_snapshots().len(),
        1 + usize::from(writer)
    );
    assert!(plane.registry.unsettled_snapshot().is_empty());
    assert!(
        probe.observed.lock().unwrap().is_empty(),
        "no Tool invocation"
    );
}

#[tokio::test]
async fn agent_writer_summary_is_bound_to_post_write_candidate_b_for_next_tool() {
    let plane = workflow_test_plane(1);
    initialize(&plane);
    let mut child = stage_workflow_child(&plane);
    let probe = Arc::new(CandidateProbe {
        status: ToolExecutionStatus::Success,
        mutate: false,
        observed: std::sync::Mutex::default(),
    });
    let context = setup_context(&plane, probe.clone());
    let mut definition = program_definition();
    definition.workspace = Some(WorkflowWorkspace {
        require_clean_parent: true,
    });
    definition.block.entry = "writer".into();
    definition.block.nodes.insert(
        "writer".into(),
        WorkflowNodeDefinition::Agent {
            profile: profile("reviewer"),
            task: "Write B and summarize".into(),
            input: BTreeMap::new(),
            output: schema(json!({"summary":{"type":"string"}}), &["summary"]),
        },
    );
    definition.block.edges.push(edge("writer", "check"));
    if let WorkflowNodeDefinition::Tool {
        arguments: WorkflowValue::Object { fields },
        ..
    } = definition.block.nodes.get_mut("check").unwrap()
    {
        fields.insert(
            "label".into(),
            WorkflowValue::Reference {
                path: vec!["writer".into(), "summary".into()],
            },
        );
    } else {
        panic!("expected fixed Tool arguments");
    }
    let runtime = workflow_runtime(&plane);
    let program = Arc::new(compile_test(definition).unwrap());
    let (_, cancellation) = workflow_cancellation();
    let task = tokio::spawn(async move {
        runtime
            .run_foreground(
                program,
                ToolCallId::new("writer-output"),
                context,
                json!({"passed":true}),
                cancellation,
            )
            .await
    });
    child.expect_delegate().await;
    let path = plane.registry.all_snapshots()[0]
        .workspace
        .logical_workspace
        .clone();
    std::fs::write(path.join("candidate"), b"post-write B").unwrap();
    child
        .send_result(
            crate::runtime::subagent::ipc::ChildResultStatus::Succeeded,
            Some(r#"{"summary":"produced B"}"#),
        )
        .await;
    assert_eq!(task.await.unwrap().unwrap()["candidate"]["version"], 1);
    assert_eq!(probe.observed.lock().unwrap().len(), 1);
    let events = plane.store.read_events(None, 256).unwrap().events;
    assert!(events.iter().any(|e| matches!(&e.event, RuntimeEvent::WorkflowCandidateInvocation { input, candidate_unchanged: true, .. } if input.version == 1)));
}

#[tokio::test]
async fn agent_inspection_failure_commits_no_successful_workflow_value() {
    agent_inspection_recovery_case(false, false).await;
}

#[tokio::test]
async fn physical_settlement_recovery_guard_preserves_post_settlement_source_edits() {
    agent_inspection_recovery_case(false, true).await;
}

#[tokio::test]
async fn agent_writer_inspection_failure_preserves_unproven_output_against_recovery_guard() {
    agent_inspection_recovery_case(true, false).await;
}

#[allow(clippy::too_many_lines)] // One gated child settlement and durable recovery transaction.
async fn agent_inspection_recovery_case(writer: bool, later_edit: bool) {
    let plane = workflow_test_plane(1);
    initialize(&plane);
    let mut child = stage_workflow_child(&plane);
    let probe = Arc::new(CandidateProbe {
        status: ToolExecutionStatus::Success,
        mutate: false,
        observed: std::sync::Mutex::default(),
    });
    let context = setup_context(&plane, probe);
    let runtime = workflow_runtime(&plane);
    let (_, cancellation) = workflow_cancellation();
    let task = tokio::spawn(async move {
        runtime
            .run_foreground(
                agent_review_program(false, false),
                ToolCallId::new("inspection-failure"),
                context,
                json!({"passed":true}),
                cancellation,
            )
            .await
    });
    child.expect_delegate().await;
    let snapshot = plane.registry.all_snapshots().pop().unwrap();
    let root = &snapshot.workspace.logical_workspace;
    let tree = snapshot.workspace.git_worktree().unwrap();
    git(root, &["branch", "unrelated-recovery"]);
    if writer {
        std::fs::write(root.join("baseline"), b"unproven writer B").unwrap();
    }
    let git_file = std::fs::read_to_string(root.join(".git")).unwrap();
    let index =
        std::path::Path::new(git_file.trim().strip_prefix("gitdir: ").unwrap()).join("index");
    let original = std::fs::read(&index).unwrap();
    std::fs::write(&index, b"invalid final index").unwrap();
    child
        .send_result(
            crate::runtime::subagent::ipc::ChildResultStatus::Succeeded,
            Some(r#"{"passed":true}"#),
        )
        .await;
    let WorkflowRunError::WorkspaceSettlement {
        workspace,
        candidate,
        ..
    } = task.await.unwrap().unwrap_err()
    else {
        panic!("missing settlement")
    };
    assert_eq!(
        workspace.unresolved_reason(),
        Some(crate::runtime::workspace::WorkspaceUnresolvedReason::PhysicalSettlement)
    );
    assert!(candidate.is_none());
    assert_eq!(
        plane.registry.all_snapshots()[0].state,
        crate::runtime::subagent::SubagentState::Failed
    );
    assert!(
        plane
            .registry
            .workflow_agent_output(&snapshot.subagent_id)
            .is_none()
    );
    let events = plane.store.read_events(None, 256).unwrap().events;
    assert!(!events.iter().any(|e| matches!(
        e.event,
        RuntimeEvent::WorkflowAgentOutputCommitted { .. }
            | RuntimeEvent::WorkflowBranchSelected { .. }
    )));
    assert!(!events.iter().any(|e| matches!(&e.event, RuntimeEvent::WorkflowNodeStarted { instance } if instance.node == "branch")));
    let (run, terminal) = events
        .iter()
        .find_map(|event| match &event.event {
            RuntimeEvent::WorkflowWorkspaceSettled {
                run_id,
                candidate: None,
                recovery_guard: Some(guard),
                ..
            } => {
                assert_eq!(guard.reference.run, *run_id);
                assert_eq!(
                    guard.reference.version, 0,
                    "only A was proven before uncertainty"
                );
                Some((run_id.clone(), event.clone()))
            }
            _ => None,
        })
        .unwrap();
    std::fs::write(index, original).unwrap();
    if later_edit {
        std::fs::write(root.join("baseline"), b"later user A prime").unwrap();
    }
    assert_eq!(git(root, &["rev-parse", "HEAD"]), tree.base_commit);
    let manager = WorkspaceManager::new(plane.dir.path().join("workspace"), &plane.runtime_root);
    let disposal = manager
        .dispose_workflow_workspace(&*plane.store, &run)
        .await;
    if writer || later_edit {
        let error = disposal.unwrap_err().to_string();
        assert!(error.contains("candidate changed"), "{error}");
        assert!(root.exists());
        assert_eq!(
            std::fs::read(root.join("baseline")).unwrap(),
            if writer {
                b"unproven writer B".as_slice()
            } else {
                b"later user A prime".as_slice()
            }
        );
        assert_eq!(
            git(root, &["rev-parse", &format!("refs/heads/{}", tree.branch)]),
            tree.base_commit
        );
        assert!(
            !plane
                .store
                .read_events(None, 256)
                .unwrap()
                .events
                .iter()
                .any(|event| matches!(
                    event.event,
                    RuntimeEvent::WorkflowWorkspaceDisposalSettled { .. }
                ))
        );
    } else {
        assert_eq!(
            disposal.unwrap(),
            crate::runtime::workspace::WorkspaceDisposalSettlement::Disposed
        );
        assert!(!root.exists());
        assert!(
            git(
                &plane.dir.path().join("workspace"),
                &["for-each-ref", &format!("refs/heads/{}", tree.branch)]
            )
            .is_empty()
        );
        assert_eq!(
            manager
                .dispose_workflow_workspace(&*plane.store, &run)
                .await
                .unwrap(),
            crate::runtime::workspace::WorkspaceDisposalSettlement::AlreadyDisposed
        );
    }
    assert_eq!(
        git(
            &plane.dir.path().join("workspace"),
            &["rev-parse", "refs/heads/unrelated-recovery"]
        ),
        tree.base_commit
    );
    let after = plane.store.read_events(None, 256).unwrap().events;
    assert_eq!(
        after
            .iter()
            .find(|event| event.event_id == terminal.event_id),
        Some(&terminal)
    );
    let run_terminal = events
        .iter()
        .find(|event| matches!(event.event, RuntimeEvent::WorkflowFailed { .. }))
        .unwrap();
    assert_eq!(
        after
            .iter()
            .find(|event| event.event_id == run_terminal.event_id),
        Some(run_terminal)
    );
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn review_dirty_candidate_accept_reject_mutation_and_cancel_gate_exact_downstream_admission()
{
    for mode in 0..5 {
        let plane = workflow_test_plane(1);
        initialize(&plane);
        let mut child = stage_workflow_child(&plane);
        let probe = Arc::new(CandidateProbe {
            status: ToolExecutionStatus::Success,
            mutate: false,
            observed: std::sync::Mutex::default(),
        });
        let mut context = setup_context(&plane, probe.clone());
        let (owner, audit, mut published) = super::human::owner(&plane);
        Arc::make_mut(context.native.as_mut().unwrap()).lifecycle =
            crate::agent::AttemptLifecycle::default().with_native_interaction(owner.clone());
        let mut definition = program_definition();
        // This exercises physical Git/child/review gates, not deadline expiry.
        // Keep the small pure-tool fixture's 100 ms deadline out of this test.
        definition.timeout_ms = 600_000;
        definition.workspace = Some(WorkflowWorkspace {
            require_clean_parent: true,
        });
        definition.block.nodes.insert(
            "implement".into(),
            WorkflowNodeDefinition::Agent {
                profile: profile("reviewer"),
                task: "Implement".into(),
                input: BTreeMap::new(),
                output: schema(json!({}), &[]),
            },
        );
        definition.block.nodes.insert(
            "review".into(),
            WorkflowNodeDefinition::Review {
                subject: WorkflowReviewSubject::Candidate {
                    value: WorkflowValue::Reference {
                        path: vec!["implement".into()],
                    },
                },
                context: vec![],
            },
        );
        definition.block.nodes.insert(
            "decision".into(),
            WorkflowNodeDefinition::Branch {
                condition: WorkflowPredicate::Boolean {
                    value: WorkflowValue::Reference {
                        path: vec!["review".into(), "accepted".into()],
                    },
                },
            },
        );
        definition.block.nodes.insert(
            "rejected".into(),
            WorkflowNodeDefinition::Return {
                output: WorkflowValue::Literal {
                    value: json!({"passed":false}),
                },
            },
        );
        definition.block.entry = "implement".into();
        definition.block.edges.extend([
            edge("implement", "review"),
            edge("review", "decision"),
            WorkflowEdgeDefinition {
                from: "decision".into(),
                to: "check".into(),
                port: Some(WorkflowPort::True),
            },
            WorkflowEdgeDefinition {
                from: "decision".into(),
                to: "rejected".into(),
                port: Some(WorkflowPort::False),
            },
        ]);
        let program = Arc::new(compile_test(definition).unwrap());
        let runtime = workflow_runtime(&plane);
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        if mode >= 3 {
            *runtime.node_frontier.lock().unwrap() =
                Some(crate::runtime::workflow::execution::NodeFrontierHook {
                    node: "check".into(),
                    entered: entered_tx,
                    release: release_rx,
                });
        }
        let (trigger, cancellation) = workflow_cancellation();
        let mut task = tokio::spawn(async move {
            runtime
                .run_foreground(
                    program,
                    ToolCallId::new("review-candidate"),
                    context,
                    json!({"passed":true}),
                    cancellation,
                )
                .await
        });
        tokio::select! {
            () = child.expect_delegate() => {},
            result = &mut task => panic!("mode {mode}: run stopped before child admission: {result:?}"),
        }
        let path = plane
            .registry
            .all_snapshots()
            .pop()
            .unwrap()
            .workspace
            .logical_workspace;
        std::fs::write(path.join("candidate"), b"candidate A dirty bytes").unwrap();
        let head = git(&path, &["rev-parse", "HEAD"]);
        child
            .send_result(
                crate::runtime::subagent::ipc::ChildResultStatus::Succeeded,
                Some("{}"),
            )
            .await;
        let request = tokio::select! {
            request = published.recv() => request.expect("review publisher remains live"),
            result = &mut task => panic!("mode {mode}: run stopped before review publication: {result:?}"),
        };
        let crate::runtime::interaction::InteractionKind::Review { review, .. } = &request.kind
        else {
            panic!("Review")
        };
        let crate::events::review::ReviewSubject::Candidate {
            reference,
            inspection_path,
        } = &review.subject
        else {
            panic!("candidate")
        };
        assert_eq!(std::path::Path::new(inspection_path), path);
        assert_eq!(reference.version, 1);
        if mode == 2 {
            std::fs::write(path.join("candidate"), b"candidate B dirty bytes").unwrap();
        }
        let response = super::human::answer(&request, mode != 1);
        let accepted = owner.respond_async(&request.id, response).await;
        if mode == 2 {
            assert!(accepted.is_err());
        } else {
            accepted.unwrap();
        }
        if mode >= 3 {
            // Accepted local data, before downstream borrow/admission. Also observe
            // terminal failure so an absent event cannot leave the test waiting.
            tokio::select! {
                entered = entered_rx => entered.expect("downstream frontier entered"),
                result = &mut task => panic!("mode {mode}: run stopped before downstream frontier: {result:?}"),
            }
            if mode == 3 {
                std::fs::write(path.join("candidate"), b"candidate B dirty bytes").unwrap();
            } else {
                trigger.cancel();
            }
            release_tx.send(()).unwrap();
        }
        let result = task.await.unwrap();
        if mode <= 1 {
            assert!(result.is_ok(), "{result:?}");
        } else {
            assert!(result.is_err());
        }
        assert_eq!(git(&path, &["rev-parse", "HEAD"]), head);
        assert_eq!(probe.observed.lock().unwrap().len(), usize::from(mode == 0));
        if mode != 0 {
            assert!(!plane.store.read_events(None, 256).unwrap().events.iter().any(|event| matches!(&event.event, RuntimeEvent::WorkflowNodeStarted { instance } if instance.node == "check")));
        }
        assert_eq!(owner.pending_count(), 0);
        assert_eq!(audit.events().len(), 2);
    }
}

fn review_reference(node: &str) -> Value {
    json!({"type":"reference","path":[node]})
}
fn repair_agent() -> Value {
    json!({"type":"agent","profile":"reviewer","task":"Produce candidate","input":{},"output":{"type":"object","properties":{},"required":[],"additionalProperties":false}})
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn workspace_independent_question_wait_does_not_borrow_or_validate_candidate() {
    let plane = workflow_test_plane(1);
    initialize(&plane);
    let mut child = stage_workflow_child(&plane);
    let (owner, _, mut published) = super::human::owner(&plane);
    let context = context_with_workspace_policy(
        &plane,
        crate::tools::native::test_ask_user_registration(),
        crate::agent::AttemptLifecycle::default().with_native_interaction(owner.clone()),
        WorkspacePolicy::GitWorktree {
            require_clean_parent: true,
        },
    );
    let question_schema = json!({"type":"object","properties":{"cancelled":{"type":"boolean"},"answers":{"type":"array","items":{"type":"object"}}},"required":["cancelled","answers"]});
    let empty = schema(json!({}), &[]);
    let definition = serde_json::from_value(json!({"description":"Independent human and writer", "workspace":{"require_clean_parent":true},"timeout_ms":600_000,"tools":[{"origin":"builtin","name":"ask_user"}],"block":{
        "input":empty,"output":empty,"entry":"parallel","nodes":{
            "parallel":{"type":"parallel","branches":{
                "question":{"input":{"type":"literal","value":{}},"block":{"input":empty,"output":question_schema,"entry":"ask","nodes":{
                    "ask":{"type":"tool","selector":{"origin":"builtin","name":"ask_user"},"arguments":{"type":"literal","value":{"questions":[{"question":"Choose","header":"Target","options":[{"label":"A","description":"First"},{"label":"B","description":"Second"}]}]}},"result":{"type":"json","part":0,"schema":question_schema}},
                    "done":{"type":"return","output":review_reference("ask")}},"edges":[{"from":"ask","to":"done"}]}},
                "writer":{"input":{"type":"literal","value":{}},"block":{"input":empty,"output":empty,"entry":"writer","nodes":{"writer":repair_agent(),"done":{"type":"return","output":review_reference("writer")}},"edges":[{"from":"writer","to":"done"}]}}
            }},"done":{"type":"return","output":{"type":"literal","value":{}}}},"edges":[{"from":"parallel","to":"done"}]}})).unwrap();
    let program = Arc::new(compile_test(definition).unwrap());
    let runtime = workflow_runtime(&plane);
    let (entered, waiting) = tokio::sync::oneshot::channel();
    let (release, gate) = tokio::sync::oneshot::channel();
    *runtime.node_frontier.lock().unwrap() =
        Some(crate::runtime::workflow::execution::NodeFrontierHook {
            node: "writer".into(),
            entered,
            release: gate,
        });
    let (_, cancellation) = workflow_cancellation();
    let task = tokio::spawn(async move {
        runtime
            .run_foreground(
                program,
                ToolCallId::new("independent-question"),
                context,
                json!({}),
                cancellation,
            )
            .await
    });
    let request = published.recv().await.unwrap();
    waiting.await.unwrap();
    assert_eq!(owner.pending_count(), 1);
    release.send(()).unwrap();
    // Native child delegation occurs only AFTER CandidateScope's real exclusive
    // borrow. This cannot happen while ask_user owns that borrow.
    child.expect_delegate().await;
    assert_eq!(owner.pending_count(), 1);
    let path = plane
        .registry
        .all_snapshots()
        .pop()
        .unwrap()
        .workspace
        .logical_workspace;
    std::fs::write(path.join("candidate"), b"B while questionnaire is pending").unwrap();
    child
        .send_result(
            crate::runtime::subagent::ipc::ChildResultStatus::Succeeded,
            Some("{}"),
        )
        .await;
    owner
        .respond_async(&request.id, super::human::answer(&request, true))
        .await
        .unwrap();
    assert!(task.await.unwrap().is_ok());
    let events = plane.store.read_events(None, 256).unwrap().events;
    assert!(!events.iter().any(|event| matches!(&event.event, RuntimeEvent::WorkflowCandidateInvocation { node, .. } if node.node == "ask")));
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn rejected_review_survives_writer_b_but_explicit_a_dependency_fails_before_start() {
    for explicit_a in [false, true] {
        let plane = workflow_test_plane(1);
        initialize(&plane);
        let mut first = stage_workflow_child(&plane);
        let mut writer = stage_workflow_child(&plane);
        let (owner, _, mut published) = super::human::owner(&plane);
        let probe = Arc::new(CandidateProbe {
            status: ToolExecutionStatus::Success,
            mutate: false,
            observed: std::sync::Mutex::default(),
        });
        let mut context = setup_context(&plane, probe);
        Arc::make_mut(context.native.as_mut().unwrap()).lifecycle =
            crate::agent::AttemptLifecycle::default().with_native_interaction(owner.clone());
        let empty = schema(json!({}), &[]);
        let result_schema = crate::runtime::workflow::review::result_schema();
        let next = if explicit_a {
            let mut node = repair_agent();
            node["input"] = json!({"old":review_reference("first")});
            node
        } else {
            json!({"type":"return","output":review_reference("review")})
        };
        let mut nodes = json!({"first":repair_agent(),"review":{"type":"review","subject":{"type":"candidate","value":review_reference("first")},"context":[]},"writer":repair_agent(),"branch":{"type":"branch","condition":{"type":"boolean","value":{"type":"reference","path":["review","accepted"]}}},"accepted":{"type":"return","output":review_reference("review")},"rejected":next});
        let mut edges = json!([{"from":"first","to":"review"},{"from":"review","to":"writer"},{"from":"writer","to":"branch"},{"from":"branch","to":"accepted","port":"true"},{"from":"branch","to":"rejected","port":"false"}]);
        if explicit_a {
            nodes["done"] = json!({"type":"return","output":review_reference("review")});
            edges
                .as_array_mut()
                .unwrap()
                .push(json!({"from":"rejected","to":"done"}));
        }
        let definition = serde_json::from_value(json!({"description":"Rejection is business data","workspace":{"require_clean_parent":true},"timeout_ms":600_000,"tools":[],"block":{"input":empty,"output":result_schema,"entry":"first","nodes":nodes,"edges":edges}})).unwrap();
        let program = Arc::new(compile_test(definition).unwrap());
        let runtime = workflow_runtime(&plane);
        let (_, cancellation) = workflow_cancellation();
        let task = tokio::spawn(async move {
            runtime
                .run_foreground(
                    program,
                    ToolCallId::new("reject-repair"),
                    context,
                    json!({}),
                    cancellation,
                )
                .await
        });
        first.expect_delegate().await;
        let path = plane
            .registry
            .all_snapshots()
            .pop()
            .unwrap()
            .workspace
            .logical_workspace;
        std::fs::write(path.join("candidate"), b"A").unwrap();
        first
            .send_result(
                crate::runtime::subagent::ipc::ChildResultStatus::Succeeded,
                Some("{}"),
            )
            .await;
        let request = published.recv().await.unwrap();
        owner
            .respond_async(&request.id, super::human::answer(&request, false))
            .await
            .unwrap();
        writer.expect_delegate().await; // Review local commit preceded writer admission.
        std::fs::write(path.join("candidate"), b"B").unwrap();
        writer
            .send_result(
                crate::runtime::subagent::ipc::ChildResultStatus::Succeeded,
                Some("{}"),
            )
            .await;
        let result = task.await.unwrap();
        if explicit_a {
            assert!(result.is_err());
        } else {
            assert_eq!(
                result.unwrap()["output"],
                json!({"accepted":false,"feedback":"Revise scope"})
            );
        }
        let events = plane.store.read_events(None, 256).unwrap().events;
        assert!(events.iter().any(|event| matches!(&event.event, RuntimeEvent::WorkflowBranchSelected { port:WorkflowPort::False, successor, .. } if successor == "rejected")));
        assert_eq!(events.iter().filter(|event| matches!(&event.event, RuntimeEvent::WorkflowNodeStarted { instance } if instance.node == "rejected")).count(), usize::from(!explicit_a));
    }
}

#[tokio::test]
async fn plan_review_candidate_check_is_audited_and_mutation_invalidates_before_settlement() {
    let plane = workflow_test_plane(1);
    initialize(&plane);
    let probe = Arc::new(CandidateProbe {
        status: ToolExecutionStatus::Success,
        mutate: false,
        observed: std::sync::Mutex::default(),
    });
    let mut context = setup_context(&plane, probe);
    let (owner, audit, mut published) = super::human::owner(&plane);
    Arc::make_mut(context.native.as_mut().unwrap()).lifecycle =
        crate::agent::AttemptLifecycle::default().with_native_interaction(owner.clone());
    let mut definition = program_definition();
    definition.timeout_ms = 600_000;
    definition.workspace = Some(WorkflowWorkspace {
        require_clean_parent: true,
    });
    definition.block.nodes.insert(
        "review".into(),
        WorkflowNodeDefinition::Review {
            subject: WorkflowReviewSubject::Plan {
                value: WorkflowValue::Reference {
                    path: vec!["args".into()],
                },
            },
            context: vec![WorkflowValue::Reference {
                path: vec!["check".into()],
            }],
        },
    );
    definition.block.edges.retain(|edge| edge.from != "check");
    definition
        .block
        .edges
        .extend([edge("check", "review"), edge("review", "branch")]);
    let program = Arc::new(compile_test(definition).unwrap());
    let runtime = workflow_runtime(&plane);
    let (_, cancellation) = workflow_cancellation();
    let task = tokio::spawn(async move {
        runtime
            .run_foreground(
                program,
                ToolCallId::new("plan-context"),
                context,
                json!({"passed":true}),
                cancellation,
            )
            .await
    });
    let request = published.recv().await.unwrap();
    let crate::runtime::interaction::InteractionKind::Review { review, .. } = &request.kind else {
        panic!("review")
    };
    let reference = review.context[0]
        .candidate
        .as_ref()
        .expect("check candidate identity published");
    assert_eq!(review.context[0].value, json!({"passed":true}));
    assert_eq!(review.candidate().unwrap(), Some(reference));
    assert!(
        serde_json::to_string(&audit.events())
            .unwrap()
            .contains(&reference.content)
    );
    let events = plane.store.read_events(None, 256).unwrap().events;
    let path = events
        .iter()
        .find_map(|event| match &event.event {
            RuntimeEvent::WorkflowWorkspaceOwned { workspace, .. } => {
                Some(workspace.logical_workspace.clone())
            }
            _ => None,
        })
        .unwrap();
    std::fs::write(path.join("baseline"), b"B while plan is pending").unwrap();
    assert!(
        owner
            .respond_async(&request.id, super::human::answer(&request, true))
            .await
            .is_err()
    );
    assert!(task.await.unwrap().is_err());
    assert!(audit.events().iter().any(|event| matches!(
        event,
        RuntimeEvent::InteractionSettled {
            settlement: crate::events::interaction::InteractionSettlement::ReviewInvalidated,
            ..
        }
    )));
    assert!(!audit.events().iter().any(|event| matches!(
        event,
        RuntimeEvent::InteractionSettled {
            settlement: crate::events::interaction::InteractionSettlement::Reviewed { .. },
            ..
        }
    )));
    assert!(!plane.store.read_events(None,256).unwrap().events.iter().any(|event| matches!(&event.event, RuntimeEvent::WorkflowNodeStarted {instance} if instance.node == "branch")));
}

#[tokio::test]
async fn review_mismatched_candidate_context_fails_before_any_prompt() {
    let plane = workflow_test_plane(1);
    initialize(&plane);
    let mut writer = stage_workflow_child(&plane);
    let probe = Arc::new(CandidateProbe {
        status: ToolExecutionStatus::Success,
        mutate: false,
        observed: std::sync::Mutex::default(),
    });
    let mut context = setup_context(&plane, probe);
    let (owner, audit, mut published) = super::human::owner(&plane);
    Arc::make_mut(context.native.as_mut().unwrap()).lifecycle =
        crate::agent::AttemptLifecycle::default().with_native_interaction(owner.clone());
    let mut definition = program_definition();
    definition.timeout_ms = 600_000;
    definition.workspace = Some(WorkflowWorkspace {
        require_clean_parent: true,
    });
    definition.block.nodes.insert(
        "writer".into(),
        serde_json::from_value(repair_agent()).unwrap(),
    );
    definition.block.nodes.insert(
        "review".into(),
        WorkflowNodeDefinition::Review {
            subject: WorkflowReviewSubject::Candidate {
                value: WorkflowValue::Reference {
                    path: vec!["writer".into()],
                },
            },
            context: vec![WorkflowValue::Reference {
                path: vec!["check".into()],
            }],
        },
    );
    definition.block.edges.retain(|edge| edge.from != "check");
    definition.block.edges.extend([
        edge("check", "writer"),
        edge("writer", "review"),
        edge("review", "branch"),
    ]);
    let program = Arc::new(compile_test(definition).unwrap());
    let runtime = workflow_runtime(&plane);
    let (_, cancellation) = workflow_cancellation();
    let task = tokio::spawn(async move {
        runtime
            .run_foreground(
                program,
                ToolCallId::new("mismatched-context"),
                context,
                json!({"passed":true}),
                cancellation,
            )
            .await
    });
    writer.expect_delegate().await; // check(A) already committed
    let path = plane
        .registry
        .all_snapshots()
        .pop()
        .unwrap()
        .workspace
        .logical_workspace;
    std::fs::write(path.join("candidate"), b"B").unwrap();
    writer
        .send_result(
            crate::runtime::subagent::ipc::ChildResultStatus::Succeeded,
            Some("{}"),
        )
        .await;
    assert!(task.await.unwrap().is_err());
    assert!(published.try_recv().is_err());
    assert!(audit.events().is_empty());
    assert_eq!(owner.pending_count(), 0);
}

#[tokio::test]
async fn cancellation_after_candidate_borrow_before_start_returns_exact_access() {
    for consumer in ["tool", "agent", "return"] {
        pre_start_candidate_case(consumer, 0, false).await;
    }
}

#[tokio::test]
async fn budget_rejection_after_candidate_borrow_returns_access_without_count_commit() {
    pre_start_candidate_case("tool", 1, false).await;
    pre_start_candidate_case("agent", 2, false).await;
}

#[tokio::test]
async fn loop_agent_budget_failure_releases_candidate_without_normal_exit() {
    pre_start_candidate_case("agent", 2, true).await;
}

#[allow(clippy::too_many_lines)]
async fn pre_start_candidate_case(consumer: &str, failure: usize, looped: bool) {
    use crate::runtime::workflow::execution::{PreStartAction, PreStartHook};
    let plane = workflow_test_plane(1);
    initialize(&plane);
    let probe = Arc::new(CandidateProbe {
        status: ToolExecutionStatus::Success,
        mutate: false,
        observed: std::sync::Mutex::default(),
    });
    let context = setup_context(&plane, probe.clone());
    let mut definition = program_definition();
    definition.workspace = Some(WorkflowWorkspace {
        require_clean_parent: true,
    });
    let mut next = match consumer {
        "tool" => definition.block.nodes["check"].clone(),
        "agent" => serde_json::from_value(repair_agent()).unwrap(),
        "return" => WorkflowNodeDefinition::Return {
            output: WorkflowValue::Reference {
                path: vec!["check".into()],
            },
        },
        _ => unreachable!(),
    };
    let dependency = WorkflowValue::Reference {
        path: vec!["check".into()],
    };
    match &mut next {
        WorkflowNodeDefinition::Tool { arguments, .. } => {
            *arguments = WorkflowValue::Object {
                fields: BTreeMap::from([
                    (
                        "passed".into(),
                        WorkflowValue::Reference {
                            path: vec!["check".into(), "passed".into()],
                        },
                    ),
                    (
                        "label".into(),
                        WorkflowValue::Literal {
                            value: json!("next"),
                        },
                    ),
                ]),
            };
        }
        WorkflowNodeDefinition::Agent { input, .. } => {
            input.insert("candidate".into(), dependency);
        }
        _ => {}
    }
    definition.block.nodes.retain(|key, _| key == "check");
    definition.block.nodes.insert("next".into(), next);
    definition.block.edges = vec![edge("check", "next")];
    if consumer != "return" {
        definition.block.nodes.insert(
            "done".into(),
            WorkflowNodeDefinition::Return {
                output: WorkflowValue::Literal {
                    value: json!({"passed":true}),
                },
            },
        );
        definition.block.edges.push(edge("next", "done"));
    }
    if looped {
        definition = super::loops::wrap_definition(definition, 3);
    }
    let program = Arc::new(compile_test(definition).unwrap());
    let execution_bound = program.execution_bound;
    let runtime = workflow_runtime(&plane);
    let (acquired, acquire) = tokio::sync::oneshot::channel();
    let (proceed, proceed_rx) = tokio::sync::oneshot::channel();
    let (released, release_rx) = tokio::sync::oneshot::channel();
    let (finish, finish_rx) = tokio::sync::oneshot::channel();
    *runtime.pre_start.lock().unwrap() = Some(PreStartHook {
        node: "next".into(),
        acquired,
        proceed: proceed_rx,
        released,
        finish: finish_rx,
    });
    let (trigger, cancellation) = workflow_cancellation();
    let task = tokio::spawn(async move {
        runtime
            .run_foreground(
                program,
                ToolCallId::new("pre-start"),
                context,
                json!({"passed":true}),
                cancellation,
            )
            .await
    });
    let (scope, reference, node, before) = acquire.await.unwrap();
    assert_eq!(before, [if looped { 3 } else { 1 }, 0]);
    // This is the actual native borrow queue, not an observation-only flag.
    let (_, fresh) = workflow_cancellation();
    let signal = fresh.child_signal();
    let mut borrower = Box::pin(scope.borrow(node, Some(&reference), &signal));
    assert!(futures_util::poll!(&mut borrower).is_pending());
    let action = match failure {
        0 => {
            trigger.cancel();
            PreStartAction::Continue
        }
        1 => PreStartAction::ExhaustSteps,
        2 => PreStartAction::ExhaustAgents,
        _ => unreachable!(),
    };
    proceed.send(action).unwrap_or_else(|_| panic!("proceed"));
    let after = release_rx.await.unwrap();
    assert_eq!(
        after,
        match failure {
            0 => before,
            1 => [execution_bound, 0],
            2 => [before[0], MAX_WORKFLOW_AGENTS],
            _ => unreachable!(),
        }
    );
    // Cleanup cleared native admitted state: the queued borrower acquires A
    // unchanged before the Workflow is allowed to settle its outer resource.
    let access = borrower.await.unwrap();
    assert_eq!(access.input(), &reference);
    assert_eq!(access.finish(true).await.unwrap(), reference);
    finish.send(()).unwrap();
    let error = task.await.unwrap().unwrap_err();
    let WorkflowRunError::WorkspaceSettlement {
        error,
        workspace,
        candidate,
        ..
    } = error
    else {
        panic!("workspace settlement")
    };
    if failure == 0 {
        assert!(
            matches!(*error, WorkflowRunError::Cancelled { .. }),
            "{error:?}"
        );
    } else {
        assert_eq!(
            *error,
            WorkflowRunError::LimitExceeded(if failure == 1 {
                WorkflowLimit::Steps
            } else {
                WorkflowLimit::Agents
            })
        );
    }
    assert_eq!(candidate.as_ref(), Some(&reference));
    assert!(workspace.unresolved_reason().is_none(), "{workspace:?}");
    assert!(!format!("{workspace:?}").contains("abandoned"));
    assert_eq!(probe.observed.lock().unwrap().len(), 1); // initial check only
    assert!(plane.registry.all_snapshots().is_empty()); // zero Agent delegation
    let events = plane.store.read_events(None, 256).unwrap().events;
    if looped {
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(
                    &event.event,
                    RuntimeEvent::WorkflowLoopIterationAdmitted { .. }
                ))
                .count(),
            1
        );
        assert!(
            !events
                .iter()
                .any(|event| matches!(&event.event, RuntimeEvent::WorkflowLoopExited { .. }))
        );
        assert!(!events.iter().any(|event| matches!(&event.event, RuntimeEvent::WorkflowNodeStarted {instance} if instance.node == "done")));
    }
    assert!(!events.iter().any(|event| matches!(&event.event,RuntimeEvent::WorkflowNodeStarted {instance} | RuntimeEvent::WorkflowNodeSettled {instance,..} if instance.node == "next")));
    assert!(!events.iter().any(|event| matches!(&event.event,RuntimeEvent::WorkflowCandidateInvocation {node,..} if node.node == "next")));
    assert!(!events.iter().any(|event| matches!(&event.event, RuntimeEvent::NativeToolInvocation { invocation_id: crate::tools::types::ToolInvocationId::Workflow { node }, fact: crate::tools::invocation::NativeInvocationFact::Started, .. } if node.node == "next")));
}

#[tokio::test]
async fn parallel_untouched_sibling_does_not_conflict_with_replaced_acceptance() {
    for idle_last in [false, true] {
        parallel_acceptance_case("replace", false, idle_last).await;
    }
}

#[tokio::test]
async fn parallel_untouched_sibling_cannot_resurrect_consumed_acceptance() {
    parallel_acceptance_case("clear", false, true).await;
}

#[tokio::test]
async fn parallel_unchanged_agent_preserves_incoming_acceptance() {
    parallel_acceptance_case("inspect", false, true).await;
}

#[tokio::test]
async fn parallel_all_unchanged_preserves_incoming_acceptance() {
    parallel_acceptance_case("unchanged", false, false).await;
}

#[tokio::test]
async fn nested_parallel_composes_replacement_relative_to_each_entry() {
    parallel_acceptance_case("replace", true, true).await;
}

#[allow(clippy::too_many_lines)]
async fn parallel_acceptance_case(mode: &str, nested: bool, idle_last: bool) {
    let plane = workflow_test_plane(1);
    initialize(&plane);
    let mut initial = stage_workflow_child(&plane);
    let mut writer = (mode != "unchanged").then(|| stage_workflow_child(&plane));
    let probe = Arc::new(CandidateProbe {
        status: ToolExecutionStatus::Success,
        mutate: false,
        observed: std::sync::Mutex::default(),
    });
    let mut context = setup_context(&plane, probe.clone());
    let (owner, _, mut published) = super::human::owner(&plane);
    Arc::make_mut(context.native.as_mut().unwrap()).lifecycle =
        crate::agent::AttemptLifecycle::default().with_native_interaction(owner.clone());
    let empty = schema(json!({}), &[]);
    let literal_return = json!({"type":"return","output":{"type":"literal","value":{}}});
    let mut right_nodes = json!({"right_done":literal_return});
    let mut right_edges = vec![];
    if mode != "unchanged" {
        right_nodes["writer"] = repair_agent();
        if mode == "replace" {
            right_nodes["review_b"] = json!({"type":"review","subject":{"type":"candidate","value":review_reference("writer")},"context":[]});
            right_edges.extend([
                json!({"from":"writer","to":"review_b"}),
                json!({"from":"review_b","to":"right_done"}),
            ]);
        } else {
            right_edges.push(json!({"from":"writer","to":"right_done"}));
        }
    }
    let mut right_block = json!({"input":empty,"output":empty,"entry":if mode == "unchanged" {"right_done"} else {"writer"},"nodes":right_nodes,"edges":right_edges});
    if nested {
        right_block = json!({"input":empty,"output":empty,"entry":"nested","nodes":{
            "nested":{"type":"parallel","branches":{
                "writer":{"input":{"type":"literal","value":{}},"block":right_block},
                "idle":{"input":{"type":"literal","value":{}},"block":{"input":empty,"output":empty,"entry":"nested_idle","nodes":{"nested_idle":literal_return},"edges":[]}}
            }},"nested_done":literal_return},"edges":[{"from":"nested","to":"nested_done"}]});
    }
    let mut definition = program_definition();
    definition.timeout_ms = 600_000;
    definition.workspace = Some(WorkflowWorkspace {
        require_clean_parent: true,
    });
    let check = definition.block.nodes["check"].clone();
    definition.block.nodes = serde_json::from_value(json!({
        "initial":repair_agent(),
        "review_a":{"type":"review","subject":{"type":"candidate","value":review_reference("initial")},"context":[]},
        "parallel":{"type":"parallel","branches":{
            "left":{"input":{"type":"literal","value":{}},"block":{"input":empty,"output":empty,"entry":"idle","nodes":{"idle":literal_return},"edges":[]}},
            "right":{"input":{"type":"literal","value":{}},"block":right_block}
        }},"check":check,"done":{"type":"return","output":review_reference("check")}
    })).unwrap();
    definition.block.entry = "initial".into();
    definition.block.edges = vec![
        edge("initial", "review_a"),
        edge("review_a", "parallel"),
        edge("parallel", "check"),
        edge("check", "done"),
    ];
    let runtime = workflow_runtime(&plane);
    let mut observations = runtime.observations.subscribe();
    let (entered, enter) = tokio::sync::oneshot::channel();
    let (release, gate) = tokio::sync::oneshot::channel();
    *runtime.node_frontier.lock().unwrap() =
        Some(crate::runtime::workflow::execution::NodeFrontierHook {
            node: "idle".into(),
            entered,
            release: gate,
        });
    let views = runtime.read_model.clone();
    let program = Arc::new(compile_test(definition).unwrap());
    let (_, cancellation) = workflow_cancellation();
    let task = tokio::spawn(async move {
        runtime
            .run_foreground(
                program,
                ToolCallId::new("parallel-acceptance"),
                context,
                json!({"passed":true}),
                cancellation,
            )
            .await
    });
    initial.expect_delegate().await;
    let path = plane
        .registry
        .all_snapshots()
        .pop()
        .unwrap()
        .workspace
        .logical_workspace;
    std::fs::write(path.join("candidate"), b"A").unwrap();
    initial
        .send_result(
            crate::runtime::subagent::ipc::ChildResultStatus::Succeeded,
            Some("{}"),
        )
        .await;
    let a = published.recv().await.unwrap();
    let crate::runtime::interaction::InteractionKind::Review { review, .. } = &a.kind else {
        panic!("review")
    };
    let expected_a = review.candidate().unwrap().unwrap().clone();
    owner
        .respond_async(&a.id, super::human::answer(&a, true))
        .await
        .unwrap();
    enter.await.unwrap();
    let mut release = Some(release);
    if !idle_last {
        release.take().unwrap().send(()).unwrap();
        observations.wait_for(|events| events.iter().any(|event| matches!(event,RuntimeEvent::WorkflowNodeSettled {instance,..} if instance.node == "idle"))).await.unwrap();
    }
    let mut expected = expected_a.clone();
    if let Some(writer) = &mut writer {
        writer.expect_delegate().await;
        if mode != "inspect" {
            std::fs::write(path.join("candidate"), b"B").unwrap();
        }
        writer
            .send_result(
                crate::runtime::subagent::ipc::ChildResultStatus::Succeeded,
                Some("{}"),
            )
            .await;
        if mode == "replace" {
            let b = published.recv().await.unwrap();
            let crate::runtime::interaction::InteractionKind::Review { review, .. } = &b.kind
            else {
                panic!("review")
            };
            expected = review.candidate().unwrap().unwrap().clone();
            assert_ne!(expected, expected_a);
            let snapshot = views.snapshot();
            let projected = &snapshot.runs[0];
            assert_eq!(projected.candidate.as_ref(), Some(&expected));
            assert!(
                projected
                    .instances
                    .iter()
                    .any(|row| row.review_accepted == Some(true)
                        && row.candidate.as_ref() == Some(&expected_a)),
                "old accepted Review is retained for A, not rebound to B"
            );
            assert!(
                projected.instances.iter().any(|row| row.node.as_ref()
                    == Some(&review.instance.node)
                    && matches!(
                        row.state,
                        crate::runtime::workflow::read_model::WorkflowState::Waiting {
                            reason: crate::runtime::workflow::read_model::WorkflowWait::Review
                        }
                    )),
                "native Review wait remains associated with its concrete branch"
            );
            owner
                .respond_async(&b.id, super::human::answer(&b, true))
                .await
                .unwrap();
        }
    }
    if idle_last {
        observations.wait_for(|events| events.iter().any(|event| matches!(event,RuntimeEvent::WorkflowNodeSettled {instance,..} if instance.node == "right_done"))).await.unwrap();
        release.take().unwrap().send(()).unwrap();
    }
    let result = task.await.unwrap();
    let events = plane.store.read_events(None, 256).unwrap().events;
    if mode == "clear" {
        expected = events
            .iter()
            .find_map(|event| match &event.event {
                RuntimeEvent::WorkflowWorkspaceSettled { candidate, .. } => candidate.clone(),
                _ => None,
            })
            .expect("settled candidate B");
        assert_ne!(expected, expected_a);
    }
    {
        assert_eq!(result.unwrap()["output"], json!({"passed":true}));
        assert_eq!(probe.observed.lock().unwrap().len(), 1);
        assert_eq!(events.iter().filter(|event| matches!(&event.event, RuntimeEvent::WorkflowNodeStarted {instance} if instance.node == "check")).count(), 1);
        let inputs = events
            .iter()
            .filter_map(|event| match &event.event {
                RuntimeEvent::WorkflowCandidateInvocation { node, input, .. }
                    if node.node == "check" =>
                {
                    Some(input)
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(inputs, vec![&expected]);
        assert_eq!(
            probe.observed.lock().unwrap()[0].1,
            if matches!(mode, "unchanged" | "inspect") {
                b"A"
            } else {
                b"B"
            }
        );
    }
    assert!(events.iter().any(|event| matches!(&event.event,RuntimeEvent::WorkflowNodeSettled {instance,outcome:WorkflowExecutionOutcome::Completed} if instance.node == "parallel")));
}

#[tokio::test]
async fn sequential_cleared_acceptance_allows_exact_b_machine_check() {
    sequential_acceptance_case("check").await;
}

#[tokio::test]
async fn cleared_acceptance_allows_repair_agent_b_to_c_before_human_review() {
    sequential_acceptance_case("repair").await;
}

#[tokio::test]
async fn cleared_acceptance_does_not_admit_explicit_stale_a_data() {
    sequential_acceptance_case("stale").await;
}

#[tokio::test]
async fn rejected_candidate_allows_predefined_repair_and_machine_check() {
    sequential_acceptance_case("reject").await;
}

#[allow(clippy::too_many_lines)]
async fn sequential_acceptance_case(mode: &str) {
    let plane = workflow_test_plane(1);
    initialize(&plane);
    let mut initial = stage_workflow_child(&plane);
    let mut writer = stage_workflow_child(&plane);
    let mut repair = (mode == "repair").then(|| stage_workflow_child(&plane));
    let probe = Arc::new(CandidateProbe {
        status: ToolExecutionStatus::Success,
        mutate: false,
        observed: std::sync::Mutex::default(),
    });
    let mut context = setup_context(&plane, probe.clone());
    let (owner, _, mut published) = super::human::owner(&plane);
    Arc::make_mut(context.native.as_mut().unwrap()).lifecycle =
        crate::agent::AttemptLifecycle::default().with_native_interaction(owner.clone());
    let mut definition = program_definition();
    definition.timeout_ms = 600_000;
    definition.workspace = Some(WorkflowWorkspace {
        require_clean_parent: true,
    });
    let check = definition.block.nodes["check"].clone();
    let mut nodes = json!({
        "initial":repair_agent(),
        "review_a":{"type":"review","subject":{"type":"candidate","value":review_reference("initial")},"context":[]},
        "writer":repair_agent(), "check":check,
        "done":{"type":"return","output":{"type":"literal","value":{"passed":true}}}
    });
    let mut edges = vec![edge("initial", "review_a"), edge("review_a", "writer")];
    if mode == "stale" {
        let mut stale = repair_agent();
        stale["input"] = json!({"old":review_reference("initial")});
        nodes["stale"] = stale;
        edges.extend([edge("writer", "stale"), edge("stale", "check")]);
    } else {
        edges.push(edge("writer", "check"));
    }
    if mode == "repair" {
        let mut next = repair_agent();
        next["input"] = json!({"current":review_reference("writer")});
        nodes["repair"] = next;
        nodes["review_c"] = json!({"type":"review","subject":{"type":"candidate","value":review_reference("repair")},"context":[]});
        edges.extend([
            edge("check", "repair"),
            edge("repair", "review_c"),
            edge("review_c", "done"),
        ]);
    } else {
        edges.push(edge("check", "done"));
    }
    definition.block.nodes = serde_json::from_value(nodes).unwrap();
    definition.block.edges = edges;
    definition.block.entry = "initial".into();
    let program = Arc::new(compile_test(definition).unwrap());
    let runtime = workflow_runtime(&plane);
    let (_, cancellation) = workflow_cancellation();
    let task = tokio::spawn(async move {
        runtime
            .run_foreground(
                program,
                ToolCallId::new("sequential-acceptance"),
                context,
                json!({"passed":true}),
                cancellation,
            )
            .await
    });
    initial.expect_delegate().await;
    let path = plane
        .registry
        .all_snapshots()
        .pop()
        .unwrap()
        .workspace
        .logical_workspace;
    std::fs::write(path.join("candidate"), b"A").unwrap();
    initial
        .send_result(
            crate::runtime::subagent::ipc::ChildResultStatus::Succeeded,
            Some("{}"),
        )
        .await;
    let request = published.recv().await.unwrap();
    let crate::runtime::interaction::InteractionKind::Review { review, .. } = &request.kind else {
        panic!("review A")
    };
    let a = review.candidate().unwrap().unwrap().clone();
    owner
        .respond_async(
            &request.id,
            super::human::answer(&request, mode != "reject"),
        )
        .await
        .unwrap();
    writer.expect_delegate().await;
    std::fs::write(path.join("candidate"), b"B").unwrap();
    writer
        .send_result(
            crate::runtime::subagent::ipc::ChildResultStatus::Succeeded,
            Some("{}"),
        )
        .await;
    let mut reviewed_c = None;
    if let Some(repair) = &mut repair {
        repair.expect_delegate().await; // B's machine check and exact B-bound repair admission completed.
        assert_eq!(probe.observed.lock().unwrap()[0].1, b"B");
        assert!(published.try_recv().is_err()); // No Human Review of B.
        std::fs::write(path.join("candidate"), b"C").unwrap();
        repair
            .send_result(
                crate::runtime::subagent::ipc::ChildResultStatus::Succeeded,
                Some("{}"),
            )
            .await;
        let request = published.recv().await.unwrap();
        let crate::runtime::interaction::InteractionKind::Review { review, .. } = &request.kind
        else {
            panic!("review C")
        };
        reviewed_c = Some(review.candidate().unwrap().unwrap().clone());
        owner
            .respond_async(&request.id, super::human::answer(&request, true))
            .await
            .unwrap();
    }
    let result = task.await.unwrap();
    let events = plane.store.read_events(None, 256).unwrap().events;
    if mode == "stale" {
        let error = result.unwrap_err();
        assert!(format!("{error}").contains("stale candidate"), "{error:?}");
        assert!(probe.observed.lock().unwrap().is_empty());
        assert!(!events.iter().any(|event| matches!(&event.event,RuntimeEvent::WorkflowNodeStarted {instance} if instance.node == "stale")));
        return;
    }
    assert_eq!(result.unwrap()["output"], json!({"passed":true}));
    let inputs = events
        .iter()
        .filter_map(|event| match &event.event {
            RuntimeEvent::WorkflowCandidateInvocation { node, input, .. }
                if node.node == "check" =>
            {
                Some(input)
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(inputs.len(), 1);
    let b = inputs[0];
    assert_ne!(b, &a);
    assert_eq!(b.version, a.version + 1);
    assert_eq!(probe.observed.lock().unwrap().len(), 1);
    assert_eq!(probe.observed.lock().unwrap()[0].1, b"B");
    assert_eq!(events.iter().filter(|event| matches!(&event.event,RuntimeEvent::WorkflowNodeStarted {instance} if instance.node == "check")).count(),1);
    let final_candidate = events
        .iter()
        .find_map(|event| match &event.event {
            RuntimeEvent::WorkflowWorkspaceSettled { candidate, .. } => candidate.as_ref(),
            _ => None,
        })
        .unwrap();
    if let Some(c) = reviewed_c {
        assert_ne!(&c, b);
        assert_eq!(c.version, b.version + 1);
        assert_eq!(final_candidate, &c);
        assert_eq!(events.iter().filter(|event| matches!(&event.event,RuntimeEvent::WorkflowNodeStarted {instance} if instance.node == "repair")).count(),1);
    } else {
        assert_eq!(final_candidate, b);
    }
}

#[tokio::test]
async fn unchanged_agent_preserves_exact_acceptance() {
    unchanged_agent_case(false).await;
}

#[tokio::test]
async fn writer_after_unchanged_agent_cannot_rebind_accepted_a_consumer() {
    unchanged_agent_case(true).await;
}

#[allow(clippy::too_many_lines)]
async fn unchanged_agent_case(writer_wins: bool) {
    use crate::runtime::workflow::execution::{PreStartAction, PreStartHook};
    let plane = workflow_test_plane(1);
    initialize(&plane);
    let mut initial = stage_workflow_child(&plane);
    let mut machine = stage_workflow_child(&plane);
    let probe = Arc::new(CandidateProbe {
        status: ToolExecutionStatus::Success,
        mutate: false,
        observed: std::sync::Mutex::default(),
    });
    let mut context = setup_context(&plane, probe.clone());
    let (owner, _, mut published) = super::human::owner(&plane);
    Arc::make_mut(context.native.as_mut().unwrap()).lifecycle =
        crate::agent::AttemptLifecycle::default().with_native_interaction(owner.clone());
    let mut definition = program_definition();
    definition.timeout_ms = 600_000;
    definition.workspace = Some(WorkflowWorkspace {
        require_clean_parent: true,
    });
    let check = definition.block.nodes["check"].clone();
    definition.block.nodes = serde_json::from_value(json!({
        "initial":repair_agent(),
        "review":{"type":"review","subject":{"type":"candidate","value":review_reference("initial")},"context":[]},
        "machine":repair_agent(),"check":check,
        "done":{"type":"return","output":{"type":"literal","value":{"passed":true}}}
    })).unwrap();
    definition.block.entry = "initial".into();
    definition.block.edges = vec![
        edge("initial", "review"),
        edge("review", "machine"),
        edge("machine", "check"),
        edge("check", "done"),
    ];
    let runtime = workflow_runtime(&plane);
    let (acquired, acquire) = tokio::sync::oneshot::channel();
    let (proceed, proceed_rx) = tokio::sync::oneshot::channel();
    let (released, release_rx) = tokio::sync::oneshot::channel();
    let (finish, finish_rx) = tokio::sync::oneshot::channel();
    *runtime.pre_start.lock().unwrap() = Some(PreStartHook {
        node: "machine".into(),
        acquired,
        proceed: proceed_rx,
        released,
        finish: finish_rx,
    });
    let program = Arc::new(compile_test(definition).unwrap());
    let (_, cancellation) = workflow_cancellation();
    let task = tokio::spawn(async move {
        runtime
            .run_foreground(
                program,
                ToolCallId::new("unchanged-agent"),
                context,
                json!({"passed":true}),
                cancellation,
            )
            .await
    });
    initial.expect_delegate().await;
    let path = plane
        .registry
        .all_snapshots()
        .pop()
        .unwrap()
        .workspace
        .logical_workspace;
    std::fs::write(path.join("candidate"), b"A").unwrap();
    initial
        .send_result(
            crate::runtime::subagent::ipc::ChildResultStatus::Succeeded,
            Some("{}"),
        )
        .await;
    let request = published.recv().await.unwrap();
    let crate::runtime::interaction::InteractionKind::Review { review, .. } = &request.kind else {
        panic!("review A")
    };
    let a = review.candidate().unwrap().unwrap().clone();
    owner
        .respond_async(&request.id, super::human::answer(&request, true))
        .await
        .unwrap();
    let (scope, input, node, _) = acquire.await.unwrap();
    assert_eq!(input, a);
    proceed
        .send(PreStartAction::Continue)
        .unwrap_or_else(|_| panic!("proceed"));
    machine.expect_delegate().await;
    // Deliberately leave the native physical candidate untouched.
    machine
        .send_result(
            crate::runtime::subagent::ipc::ChildResultStatus::Succeeded,
            Some("{}"),
        )
        .await;
    release_rx.await.unwrap(); // Agent physical settlement and local result commit completed.
    let (_, fresh) = workflow_cancellation();
    let access = scope
        .borrow(node, Some(&a), &fresh.child_signal())
        .await
        .unwrap();
    assert_eq!(access.input(), &a); // Authoritative post-Agent candidate is still exact A.
    let post = if writer_wins {
        std::fs::write(path.join("candidate"), b"B").unwrap();
        access.finish(false).await.unwrap() // Legitimate native writer wins before next admission.
    } else {
        access.finish(true).await.unwrap()
    };
    assert_eq!(post == a, !writer_wins);
    finish.send(()).unwrap();
    let result = task.await.unwrap();
    let events = plane.store.read_events(None, 256).unwrap().events;
    assert_eq!(events.iter().filter(|event|matches!(&event.event,RuntimeEvent::WorkflowNodeStarted {instance} if instance.node == "machine")).count(),1);
    let starts = events.iter().filter(|event|matches!(&event.event,RuntimeEvent::WorkflowNodeStarted {instance} if instance.node == "check")).count();
    if writer_wins {
        let error = result.unwrap_err();
        assert!(format!("{error}").contains("stale candidate"), "{error:?}");
        assert_eq!(starts, 0);
        assert!(probe.observed.lock().unwrap().is_empty());
    } else {
        assert_eq!(result.unwrap()["output"], json!({"passed":true}));
        assert_eq!(starts, 1);
        let inputs = events
            .iter()
            .filter_map(|event| match &event.event {
                RuntimeEvent::WorkflowCandidateInvocation { node, input, .. }
                    if node.node == "check" =>
                {
                    Some(input)
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(inputs, vec![&a]);
        assert_eq!(probe.observed.lock().unwrap()[0].1, b"A");
    }
}

#[tokio::test]
async fn parallel_none_entry_review_then_mutation_conflicts_before_downstream() {
    none_entry_review_mutation_case(false).await;
}

#[tokio::test]
async fn nested_none_entry_mutation_exports_clear_against_sibling_review() {
    none_entry_review_mutation_case(true).await;
}

#[allow(clippy::too_many_lines)]
async fn none_entry_review_mutation_case(nested: bool) {
    use crate::runtime::workflow::execution::{NodeFrontierHook, PreStartAction, PreStartHook};
    let plane = workflow_test_plane(1);
    initialize(&plane);
    let mut initial = stage_workflow_child(&plane);
    let mut writer = stage_workflow_child(&plane);
    let probe = Arc::new(CandidateProbe {
        status: ToolExecutionStatus::Success,
        mutate: false,
        observed: std::sync::Mutex::default(),
    });
    let mut context = setup_context(&plane, probe.clone());
    let (owner, _, mut published) = super::human::owner(&plane);
    Arc::make_mut(context.native.as_mut().unwrap()).lifecycle =
        crate::agent::AttemptLifecycle::default().with_native_interaction(owner.clone());
    let empty = schema(json!({}), &[]);
    let input = json!({"type":"reference","path":["args"]});
    let done = json!({"type":"return","output":{"type":"literal","value":{}}});
    let mut agent = repair_agent();
    agent["input"] = json!({"candidate":input});
    let mut writer_block = json!({"input":empty,"output":empty,"entry":"writer","nodes":{"writer":agent,"writer_done":done},"edges":[{"from":"writer","to":"writer_done"}]});
    if nested {
        writer_block = json!({"input":empty,"output":empty,"entry":"inner","nodes":{
            "inner":{"type":"parallel","branches":{
                "writer":{"input":input,"block":writer_block},
                "idle":{"input":{"type":"literal","value":{}},"block":{"input":empty,"output":empty,"entry":"idle","nodes":{"idle":done},"edges":[]}}
            }},"inner_done":done},"edges":[{"from":"inner","to":"inner_done"}]});
    }
    let mut definition = program_definition();
    definition.workspace = Some(WorkflowWorkspace {
        require_clean_parent: true,
    });
    definition.timeout_ms = 600_000;
    let check = definition.block.nodes["check"].clone();
    definition.block.entry = "initial".into();
    definition.block.nodes = serde_json::from_value(json!({
        "initial":repair_agent(),
        "parallel":{"type":"parallel","branches":{
            "review":{"input":review_reference("initial"),"block":{"input":empty,"output":empty,"entry":"review","nodes":{
                "review":{"type":"review","subject":{"type":"candidate","value":input},"context":[]},"review_done":done
            },"edges":[{"from":"review","to":"review_done"}]}},
            "writer":{"input":review_reference("initial"),"block":writer_block}
        }},"check":check,"done":{"type":"return","output":{"type":"literal","value":{"passed":true}}}
    })).unwrap();
    definition.block.edges = vec![
        edge("initial", "parallel"),
        edge("parallel", "check"),
        edge("check", "done"),
    ];
    let runtime = workflow_runtime(&plane);
    let (entered, enter) = tokio::sync::oneshot::channel();
    let (release, gate) = tokio::sync::oneshot::channel();
    *runtime.node_frontier.lock().unwrap() = Some(NodeFrontierHook {
        node: "writer".into(),
        entered,
        release: gate,
    });
    let (acquired, acquire) = tokio::sync::oneshot::channel();
    let (proceed, proceed_rx) = tokio::sync::oneshot::channel();
    let (released, release_rx) = tokio::sync::oneshot::channel();
    let (finish, finish_rx) = tokio::sync::oneshot::channel();
    *runtime.pre_start.lock().unwrap() = Some(PreStartHook {
        node: "writer".into(),
        acquired,
        proceed: proceed_rx,
        released,
        finish: finish_rx,
    });
    let program = Arc::new(compile_test(definition).unwrap());
    let (_, cancellation) = workflow_cancellation();
    let task = tokio::spawn(async move {
        runtime
            .run_foreground(
                program,
                ToolCallId::new("none-review-mutation"),
                context,
                json!({"passed":true}),
                cancellation,
            )
            .await
    });
    initial.expect_delegate().await;
    let path = plane
        .registry
        .all_snapshots()
        .pop()
        .unwrap()
        .workspace
        .logical_workspace;
    std::fs::write(path.join("candidate"), b"A").unwrap();
    initial
        .send_result(
            crate::runtime::subagent::ipc::ChildResultStatus::Succeeded,
            Some("{}"),
        )
        .await;
    let request = published.recv().await.unwrap();
    let crate::runtime::interaction::InteractionKind::Review { review, .. } = &request.kind else {
        panic!("review A")
    };
    let a = review.candidate().unwrap().unwrap().clone();
    owner
        .respond_async(&request.id, super::human::answer(&request, true))
        .await
        .unwrap();
    enter.await.unwrap(); // Block input validation can also wait behind the Review freeze.
    release.send(()).unwrap();
    let (scope, pre, node, _) = acquire.await.unwrap(); // Real borrow completes only after freeze release.
    assert_eq!(pre, a);
    proceed
        .send(PreStartAction::Continue)
        .unwrap_or_else(|_| panic!("proceed"));
    writer.expect_delegate().await;
    std::fs::write(path.join("candidate"), b"B").unwrap();
    writer
        .send_result(
            crate::runtime::subagent::ipc::ChildResultStatus::Succeeded,
            Some("{}"),
        )
        .await;
    release_rx.await.unwrap(); // Native Agent settlement and branch-local result commit completed.
    let (_, fresh) = workflow_cancellation();
    let access = scope
        .borrow(node, None, &fresh.child_signal())
        .await
        .unwrap();
    let post = access.input().clone();
    assert_ne!(pre, post);
    assert_eq!(access.finish(true).await.unwrap(), post);
    finish.send(()).unwrap();
    let error = task.await.unwrap().unwrap_err();
    assert!(
        format!("{error}").contains("conflicting candidate acceptance transitions"),
        "{error:?}"
    );
    let events = plane.store.read_events(None, 256).unwrap().events;
    assert!(!events.iter().any(|event|matches!(&event.event,RuntimeEvent::WorkflowNodeStarted {instance} if instance.node == "check")));
    assert!(!events.iter().any(|event|matches!(&event.event,RuntimeEvent::WorkflowCandidateInvocation {node,..} if node.node == "check")));
    assert!(probe.observed.lock().unwrap().is_empty());
    assert!(events.iter().any(|event|matches!(&event.event,RuntimeEvent::WorkflowWorkspaceSettled {candidate:Some(candidate),..} if candidate == &post)));
}
