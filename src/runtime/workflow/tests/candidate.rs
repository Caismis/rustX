//! WF-03 cross-owner regressions using the real registry/process settlement.
use super::*;

fn git(root: &std::path::Path, args: &[&str]) {
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
}
fn initialize(plane: &WorkflowTestPlane) {
    let root = plane.dir.path().join("workspace");
    git(&root, &["init"]);
    std::fs::write(root.join("baseline"), b"parent\n").unwrap();
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "baseline"]);
}
fn candidate_program(agent: bool) -> Arc<WorkflowProgram> {
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
    Arc::new(compile_test(definition).unwrap())
}

struct CandidateProbe {
    status: ToolExecutionStatus,
    mutate: bool,
    observed: std::sync::Mutex<Vec<(std::path::PathBuf, Vec<u8>)>>,
}
impl ToolExecutor for CandidateProbe {
    fn honors_workspace(&self) -> bool {
        true
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
    fn honors_workspace(&self) -> bool {
        true
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
