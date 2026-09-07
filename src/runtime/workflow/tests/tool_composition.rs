//! Cross-owner WF-02 composition. Every wait below names an observed frontier.
use super::*;

#[test]
fn typed_failure_aggregation_uses_definition_keys_with_unknown_dominance() {
    let failure = |status| WorkflowRunError::ToolFailed {
        node: "leaf".into(),
        status,
    };
    let denied = ToolExecutionStatus::Denied {
        reason: "alpha".into(),
    };
    let unknown = ToolExecutionStatus::OutcomeUnknown {
        detail: "remote effect".into(),
    };
    let mut failures = BTreeMap::from([
        ("zeta".into(), failure(ToolExecutionStatus::TimedOut)),
        ("alpha".into(), failure(denied.clone())),
    ]);
    let joined = |failures| WorkflowRunError::ParallelFailed {
        node: "join".into(),
        failures,
    };
    assert_eq!(joined(failures.clone()).execution_status(), denied);
    failures.insert("zeta".into(), failure(unknown.clone()));
    assert_eq!(joined(failures).execution_status(), unknown);
}

#[tokio::test]
async fn simultaneous_leaf_completion_outer_deadline_and_cancellation_have_one_terminal() {
    for cancel in [false, true] {
        let plane = workflow_test_plane(1);
        let runtime = workflow_runtime(&plane);
        let mut probe = Probe::new(ToolExecutionStatus::Success);
        let release = Arc::new(tokio::sync::Notify::new());
        Arc::get_mut(&mut probe).unwrap().release = Some(release.clone());
        let mut context = context(
            &plane,
            probe.clone(),
            crate::agent::AttemptLifecycle::default(),
        );
        let clock = Arc::new(crate::runtime::ManualMonotonicClock::new());
        Arc::make_mut(context.native.as_mut().unwrap()).clock = clock.clone();
        let mut started = probe.started.subscribe();
        let (trigger, cancellation) = workflow_cancellation();
        let task = tokio::spawn(run_outer(runtime, program(), context, cancellation));
        started.wait_for(|count| *count == 1).await.unwrap();
        // Current-thread runtime: no await between making every contender
        // ready. The next poll must choose cancellation > hard > completion.
        release.notify_one();
        clock.advance(100);
        if cancel {
            trigger.cancel();
        }
        let (result, _) = task.await.unwrap();
        if cancel {
            assert!(matches!(
                result.status,
                ToolExecutionStatus::Cancelled { .. }
            ));
        } else {
            assert_eq!(result.status, ToolExecutionStatus::TimedOut);
        }
        let events = plane.store.read_events(None, 128).unwrap().events;
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event.event, RuntimeEvent::WorkflowNodeSettled { .. }))
                .count(),
            1
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event.event, RuntimeEvent::WorkflowFailed { .. }))
                .count(),
            1
        );
        let facts = events
            .iter()
            .filter_map(|event| match &event.event {
                RuntimeEvent::NativeToolInvocation { fact, .. } => Some(fact),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            facts
                .iter()
                .filter(|fact| matches!(fact, NativeInvocationFact::Completed { .. }))
                .count(),
            1
        );
        let expected_cause = if cancel {
            crate::tools::deadline::ToolCancellationCause::Attempt(
                CancellationReason::UserRequested,
            )
        } else {
            crate::tools::deadline::ToolCancellationCause::Deadline(
                crate::tools::deadline::ToolDeadlineKind::Hard,
            )
        };
        let causes = facts
            .iter()
            .filter_map(|fact| match fact {
                NativeInvocationFact::Lifecycle {
                    fact: crate::tools::invocation::InvocationFact::CancellationRequested { cause },
                } => Some(*cause),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(causes, vec![expected_cause]);
        assert!(
            matches!(facts.last(), Some(NativeInvocationFact::Completed { status }) if *status == result.status)
        );
    }
}

#[tokio::test]
async fn committed_completion_is_not_overwritten_by_later_cancel_or_deadline() {
    let plane = workflow_test_plane(1);
    let runtime = workflow_runtime(&plane);
    let probe = Probe::new(ToolExecutionStatus::Success);
    let mut context = context(&plane, probe, crate::agent::AttemptLifecycle::default());
    let clock = Arc::new(crate::runtime::ManualMonotonicClock::new());
    Arc::make_mut(context.native.as_mut().unwrap()).clock = clock.clone();
    let (trigger, cancellation) = workflow_cancellation();
    // The shared owner's narrow hook runs only AFTER physical completion won.
    let hook: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
        trigger.cancel();
        clock.advance(100);
    });
    let executor = crate::tools::native::test_workflow_executor(runtime, program());
    let (result, facts) = drive_executor(executor, context, cancellation, Some(hook)).await;
    assert_eq!(result.status, ToolExecutionStatus::Success);
    assert!(facts.is_empty());
    let events = plane.store.read_events(None, 128).unwrap().events;
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event.event, RuntimeEvent::WorkflowCompleted { .. }))
            .count(),
        1
    );
    assert!(!events.iter().any(|event| matches!(
        event.event,
        RuntimeEvent::WorkflowFailed { .. } | RuntimeEvent::WorkflowCancelled { .. }
    )));
}

#[test]
fn projection_rejects_oversized_or_missing_parts_without_parsing_text() {
    let mut result = terminal(ToolExecutionStatus::Success);
    let node = test_instance("check", "check");
    result.content = vec![ToolResultContent::Text(
        crate::message::content::TextBlock {
            text: "x".repeat(MAX_VALUE_BYTES + 1),
        },
    )];
    assert!(
        WorkflowToolResult::Text { part: 0 }
            .project(&result, &node)
            .is_err()
    );
    result.content = vec![ToolResultContent::Json {
        value: json!("x".repeat(MAX_VALUE_BYTES + 1)),
    }];
    assert!(
        WorkflowToolResult::Json {
            part: 0,
            schema: json!({"type":"string"})
        }
        .project(&result, &node)
        .is_err()
    );
    assert!(
        WorkflowToolResult::Text { part: 1 }
            .project(&result, &node)
            .is_err()
    );
}

struct BrokenComposite(Arc<Probe>);
impl ToolExecutor for BrokenComposite {
    fn progress_capability(&self) -> crate::tools::deadline::ToolProgressCapability {
        crate::tools::deadline::ToolProgressCapability::None
    }
    fn start<'a>(
        &'a self,
        invocation: ToolInvocation,
        context: ToolExecutionContext<'a>,
    ) -> ToolExecutionHandle<'a> {
        self.0.start(invocation, context)
    }
}

#[tokio::test]
async fn a_broken_composite_without_owned_children_still_has_a_finite_control_guard() {
    let plane = workflow_test_plane(1);
    let mut probe = Probe::new(ToolExecutionStatus::Success);
    Arc::get_mut(&mut probe).unwrap().release = Some(Arc::new(tokio::sync::Notify::new()));
    Arc::get_mut(&mut probe).unwrap().settled = Some(Arc::new(tokio::sync::Notify::new()));
    let mut context = context(
        &plane,
        probe.clone(),
        crate::agent::AttemptLifecycle::default(),
    );
    let clock = Arc::new(crate::runtime::ManualMonotonicClock::new());
    Arc::make_mut(context.native.as_mut().unwrap()).clock = clock.clone();
    let mut started = probe.started.subscribe();
    let mut cancelled = probe.cancelled.subscribe();
    let (_, cancellation) = workflow_cancellation();
    let task = tokio::spawn(drive_executor(
        (
            Arc::new(BrokenComposite(probe)),
            crate::tools::deadline::ForegroundPolicy::Composite {
                total: crate::tools::deadline::ToolExecutionDeadlinePolicy::new(
                    std::time::Duration::from_millis(100),
                    None,
                ),
            },
        ),
        context,
        cancellation,
        None,
    ));
    started.wait_for(|count| *count == 1).await.unwrap();
    clock.advance(100);
    cancelled.wait_for(|value| *value).await.unwrap(); // control guard armed with no delegated owners
    clock.advance(30_000);
    let (result, facts) = task.await.unwrap();
    assert!(matches!(
        result.status,
        ToolExecutionStatus::OutcomeUnknown { .. }
    ));
    assert!(facts.iter().any(|fact| matches!(
        fact,
        crate::tools::invocation::InvocationFact::SettlementControlFailed { .. }
    )));
}

#[tokio::test]
async fn every_native_non_success_survives_the_actual_outer_adapter_once() {
    for status in [
        ToolExecutionStatus::Failed {
            error: "launch failed".into(),
        },
        ToolExecutionStatus::Denied {
            reason: "policy denied".into(),
        },
        ToolExecutionStatus::Cancelled {
            reason: CancellationReason::UserRequested,
            phase: ToolCancellationPhase::DuringExecution,
        },
        ToolExecutionStatus::TimedOut,
        ToolExecutionStatus::OutcomeUnknown {
            detail: "remote effect unresolved".into(),
        },
    ] {
        let plane = workflow_test_plane(1);
        let runtime = workflow_runtime(&plane);
        let probe = Probe::new(status.clone());
        let context = context(&plane, probe, crate::agent::AttemptLifecycle::default());
        let (_, cancellation) = workflow_cancellation();
        let (result, _) = run_outer(runtime, program(), context, cancellation).await;
        assert_eq!(result.status, status);
        assert!(
            result.content.is_empty(),
            "failed execution cannot commit business output"
        );
        let events = plane.store.read_events(None, 128).unwrap().events;
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event.event, RuntimeEvent::WorkflowNodeSettled { .. }))
                .count(),
            1
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event.event, RuntimeEvent::WorkflowFailed { .. }))
                .count(),
            1
        );
    }
}

#[tokio::test]
async fn fixed_admission_rejects_orchestration_background_and_composite_leaves() {
    use crate::capabilities::selection::ToolSelector;
    for name in [
        "subagent",
        "execution",
        "ask_user",
        "todo",
        "background",
        "composite",
    ] {
        let plane = workflow_test_plane(1);
        let runtime = workflow_runtime(&plane);
        let probe = Probe::new(ToolExecutionStatus::Success);
        let mut leaf = definition();
        leaf.name = name.into();
        leaf.id = crate::runtime::identity::ToolId::new(format!("tool-{name}"));
        if name == "background" {
            leaf.execution_policy = ToolExecutionPolicy::BackgroundOnly;
        }
        let mut registration = ToolRegistration::plain(leaf, probe.clone());
        if name == "composite" {
            let mut registry = crate::tools::executor::ToolRegistry::new();
            registry
                .register_with_activation_metadata(
                    registration.definition,
                    registration.executor,
                    registration.normalizer,
                    false,
                    crate::tools::deadline::ForegroundPolicy::Composite {
                        total: crate::tools::deadline::ToolExecutionDeadlinePolicy::default(),
                    },
                )
                .unwrap();
            registration = registry.registrations().remove(0);
        }
        let context = context_with_registration(
            &plane,
            registration,
            crate::agent::AttemptLifecycle::default(),
        );
        let selector = ToolSelector::Builtin { name: name.into() };
        let mut definition = program_definition();
        definition.tools = BTreeSet::from([selector.clone()]);
        if let WorkflowNodeDefinition::Tool {
            selector: target, ..
        } = definition.block.nodes.get_mut("check").unwrap()
        {
            *target = selector;
        }
        let program = Arc::new(compile_test(definition).unwrap());
        let (_, cancellation) = workflow_cancellation();
        assert!(
            runtime
                .run_foreground(
                    program,
                    ToolCallId::new("outer"),
                    context,
                    json!({"passed":false}),
                    cancellation
                )
                .await
                .is_err(),
            "{name}"
        );
        assert_eq!(probe.starts.load(Ordering::SeqCst), 0);
    }
}

#[test]
fn composite_deadline_is_trusted_finite_configuration() {
    for timeout in [0, 86_400_001, u64::MAX] {
        let mut definition = program_definition();
        definition.timeout_ms = timeout;
        assert!(compile_test(definition).is_err());
    }
    let mut definition = program_definition();
    definition.timeout_ms = 86_400_000;
    assert_eq!(compile_test(definition).unwrap().timeout_ms(), 86_400_000);
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // The two deliberately opposite terminal paths share one gated scenario.
async fn mixed_parallel_all_settles_in_key_order_without_internal_history() {
    for status in [
        ToolExecutionStatus::Success,
        ToolExecutionStatus::OutcomeUnknown {
            detail: "remote effect unresolved".into(),
        },
    ] {
        let plane = workflow_test_plane(1);
        let mut child = stage_workflow_child(&plane);
        let runtime = workflow_runtime(&plane);
        let mut observations = runtime.observations.subscribe();
        let probe = Probe::new(status.clone());
        let context = context(&plane, probe, crate::agent::AttemptLifecycle::default());
        let mut definition = program_definition();
        let leaf = definition.block.clone();
        let mut agent = leaf.clone();
        agent.entry = "agent".into();
        agent.nodes = BTreeMap::from([
            (
                "agent".into(),
                WorkflowNodeDefinition::Agent {
                    profile: profile("reviewer"),
                    task: "Return a boolean".into(),
                    input: BTreeMap::new(),
                    output: leaf.output.clone(),
                },
            ),
            (
                "done".into(),
                WorkflowNodeDefinition::Return {
                    output: WorkflowValue::Reference {
                        path: vec!["agent".into()],
                    },
                },
            ),
        ]);
        agent.edges = vec![edge("agent", "done")];
        let input = WorkflowValue::Literal {
            value: json!({"passed":false}),
        };
        definition.block.entry = "fanout".into();
        definition.block.output = schema(
            json!({"alpha":leaf.output, "beta":agent.output}),
            &["alpha", "beta"],
        );
        definition.block.nodes = BTreeMap::from([
            (
                "fanout".into(),
                WorkflowNodeDefinition::Parallel {
                    branches: BTreeMap::from([
                        (
                            "alpha".into(),
                            WorkflowBranch {
                                input: input.clone(),
                                block: leaf,
                            },
                        ),
                        (
                            "beta".into(),
                            WorkflowBranch {
                                input,
                                block: agent,
                            },
                        ),
                    ]),
                },
            ),
            (
                "done".into(),
                WorkflowNodeDefinition::Return {
                    output: WorkflowValue::Reference {
                        path: vec!["fanout".into()],
                    },
                },
            ),
        ]);
        definition.block.edges = vec![edge("fanout", "done")];
        let program = Arc::new(compile_test(definition).unwrap());
        let (_, cancellation) = workflow_cancellation();
        let task = tokio::spawn(async move {
            runtime
                .run_foreground(
                    program,
                    ToolCallId::new("outer"),
                    context,
                    json!({"passed":false}),
                    cancellation,
                )
                .await
        });
        child.expect_delegate().await; // beta is admitted and held before terminal output
        observations.wait_for(|events| events.iter().any(|event| matches!(event, RuntimeEvent::WorkflowBlockSettled {instance,..} if instance.definition.blocks == ["fanout","alpha"]))).await.unwrap();
        assert!(
            !task.is_finished(),
            "even an unknown alpha must drain the owned beta"
        );
        child
            .send_result(
                crate::runtime::subagent::ipc::ChildResultStatus::Succeeded,
                Some(r#"{"passed":true}"#),
            )
            .await;
        let result = task.await.unwrap();
        if status == ToolExecutionStatus::Success {
            assert_eq!(
                result.unwrap(),
                json!({"alpha":{"passed":false},"beta":{"passed":true}})
            );
        } else {
            assert_eq!(result.unwrap_err().execution_status(), status);
        }
        let events = plane.store.read_events(None, 256).unwrap().events;
        assert!(!events.iter().any(|event| matches!(
            event.event,
            RuntimeEvent::AssistantMessageCommitted { .. }
                | RuntimeEvent::ToolMessageCommitted { .. }
        )));
        let settled = events
            .iter()
            .filter(|event| {
                matches!(
                    event.event,
                    RuntimeEvent::WorkflowCompleted { .. } | RuntimeEvent::WorkflowFailed { .. }
                )
            })
            .count();
        assert_eq!(settled, 1, "one outer run terminal after all branches");
    }
}

#[tokio::test]
async fn nested_control_guard_failure_preserves_unknown_instead_of_outer_timeout() {
    let plane = workflow_test_plane(1);
    let runtime = workflow_runtime(&plane);
    let mut probe = Probe::new(ToolExecutionStatus::Success);
    Arc::get_mut(&mut probe).unwrap().release = Some(Arc::new(tokio::sync::Notify::new()));
    Arc::get_mut(&mut probe).unwrap().settled = Some(Arc::new(tokio::sync::Notify::new())); // deliberately broken leaf settlement plane
    let mut context = context(
        &plane,
        probe.clone(),
        crate::agent::AttemptLifecycle::default(),
    );
    let clock = Arc::new(crate::runtime::ManualMonotonicClock::new());
    Arc::make_mut(context.native.as_mut().unwrap()).clock = clock.clone();
    let mut started = probe.started.subscribe();
    let mut cancelled = probe.cancelled.subscribe();
    let (_, cancellation) = workflow_cancellation();
    let task = tokio::spawn(run_outer(runtime, program(), context, cancellation));
    started.wait_for(|count| *count == 1).await.unwrap();
    clock.advance(100);
    cancelled.wait_for(|value| *value).await.unwrap(); // leaf cancellation observed; settlement guard armed
    assert!(!task.is_finished());
    clock.advance(30_000);
    let (result, facts) = task.await.unwrap();
    assert!(matches!(
        result.status,
        ToolExecutionStatus::OutcomeUnknown { .. }
    ));
    assert!(
        !facts.iter().any(|fact| matches!(
            fact,
            crate::tools::invocation::InvocationFact::SettlementControlFailed { .. }
        )),
        "composite consumes descendant evidence, not its own competing guard"
    );
    let events = plane.store.read_events(None, 256).unwrap().events;
    assert_eq!(events.iter().filter(|event| matches!(&event.event,RuntimeEvent::NativeToolInvocation {fact:NativeInvocationFact::Lifecycle {fact:crate::tools::invocation::InvocationFact::SettlementControlFailed {..}},..})).count(),1);
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(
                &event.event,
                RuntimeEvent::NativeToolInvocation {
                    fact: NativeInvocationFact::Completed {
                        status: ToolExecutionStatus::OutcomeUnknown { .. }
                    },
                    ..
                }
            ))
            .count(),
        1
    );
}

#[tokio::test]
async fn leaf_deadline_remains_independent_of_composite_total() {
    let plane = workflow_test_plane(1);
    let runtime = workflow_runtime(&plane);
    let mut probe = Probe::new(ToolExecutionStatus::Success);
    Arc::get_mut(&mut probe).unwrap().release = Some(Arc::new(tokio::sync::Notify::new()));
    let mut context = context(
        &plane,
        probe.clone(),
        crate::agent::AttemptLifecycle::default(),
    );
    let clock = Arc::new(crate::runtime::ManualMonotonicClock::new());
    let services = Arc::make_mut(context.native.as_mut().unwrap());
    services.clock = clock.clone();
    services.leaf_policy = crate::tools::deadline::ToolExecutionDeadlinePolicy::new(
        std::time::Duration::from_millis(20),
        None,
    );
    let mut started = probe.started.subscribe();
    let (_, cancellation) = workflow_cancellation();
    let task = tokio::spawn(run_outer(runtime, program(), context, cancellation));
    started.wait_for(|count| *count == 1).await.unwrap();
    clock.advance(20); // outer still has 80ms left; leaf alone expires
    let (result, facts) = task.await.unwrap();
    assert_eq!(result.status, ToolExecutionStatus::TimedOut);
    assert!(
        facts.is_empty(),
        "outer physical completion retains the leaf's typed timeout"
    );
}
