//! WF-02 boundary tests. Gates identify the interleaving; no sleeps establish order.
#![cfg(unix)]
#[path = "candidate.rs"]
mod candidate;
#[path = "tool_composition.rs"]
mod composition;
#[path = "human.rs"]
mod human;
use super::*;
use crate::tools::executor::{
    ToolExecutionContext, ToolExecutionHandle, ToolExecutor, ToolRegistration,
};
use crate::tools::invocation::{NativeInvocationFact, NativeInvocationServices, terminal};
use crate::tools::types::*;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Probe {
    starts: AtomicUsize,
    received: std::sync::Mutex<Vec<ToolInvocation>>,
    status: ToolExecutionStatus,
    started: tokio::sync::watch::Sender<usize>,
    cancelled: tokio::sync::watch::Sender<bool>,
    release: Option<Arc<tokio::sync::Notify>>,
    settled: Option<Arc<tokio::sync::Notify>>,
}

impl Probe {
    fn new(status: ToolExecutionStatus) -> Arc<Self> {
        Arc::new(Self {
            starts: AtomicUsize::new(0),
            received: std::sync::Mutex::default(),
            status,
            started: tokio::sync::watch::Sender::new(0),
            cancelled: tokio::sync::watch::Sender::new(false),
            release: None,
            settled: None,
        })
    }
}

impl ToolExecutor for Probe {
    fn start<'a>(
        &'a self,
        invocation: ToolInvocation,
        context: ToolExecutionContext<'a>,
    ) -> ToolExecutionHandle<'a> {
        let cancellation = context.cancellation.clone();
        ToolExecutionHandle::settled_by_operation(
            Box::pin(async move {
                self.received.lock().unwrap().push(invocation.clone());
                context.progress.report(ToolProgress {
                    message: Some("physical check started".into()),
                    ..ToolProgress::default()
                });
                let count = self.starts.fetch_add(1, Ordering::SeqCst) + 1;
                self.started.send_replace(count); // proves the first physical poll happened
                if let Some(release) = &self.release {
                    tokio::select! {
                        biased;
                        () = cancellation.cancelled() => {
                            self.cancelled.send_replace(true); // proves native cancellation was observed
                            if let Some(settled) = &self.settled { settled.notified().await; }
                            return terminal(ToolExecutionStatus::Cancelled { reason: cancellation.reason(), phase: ToolCancellationPhase::DuringExecution });
                        },
                        () = release.notified() => {},
                    }
                }
                let mut result = terminal(self.status.clone());
                result.content = vec![
                    ToolResultContent::Text(crate::message::content::TextBlock {
                        text: "passed=true is only a log".into(),
                    }),
                    ToolResultContent::Json {
                        value: json!({"passed": invocation.arguments["passed"]}),
                    },
                ];
                result
            }),
            context.cancellation,
        )
    }
    fn progress_capability(&self) -> crate::tools::deadline::ToolProgressCapability {
        crate::tools::deadline::ToolProgressCapability::None
    }
}

fn definition() -> ToolDefinition {
    ToolDefinition {
        id: crate::runtime::identity::ToolId::new("tool-check"),
        name: "check".into(),
        description: "fixed check".into(),
        input_schema: schema(
            json!({"passed":{"type":"boolean"},"label":{"type":"string"}}),
            &["passed", "label"],
        ),
        execution_policy: ToolExecutionPolicy::ModelSelectable,
        concurrency_policy: ToolConcurrencyPolicy::Sequential,
        approval_policy: ToolApprovalPolicy::Never,
        replay_policy: ToolReplayPolicy::Never,
        origin: ToolOrigin::Builtin,
    }
}

fn program() -> Arc<WorkflowProgram> {
    Arc::new(compile_test(program_definition()).unwrap())
}

fn program_definition() -> WorkflowDefinition {
    serde_json::from_value(json!({
        "description":"fixed verification", "tools":[{"origin":"builtin","name":"check"}], "timeout_ms":100,
        "block": {
            "input":schema(json!({"passed":{"type":"boolean"}}), &["passed"]),
            "output":schema(json!({"passed":{"type":"boolean"}}), &["passed"]),
            "entry":"check",
            "nodes": {
                "check":{"type":"tool","selector":{"origin":"builtin","name":"check"},
                    "arguments":{"type":"object","fields":{
                        "passed":{"type":"reference","path":["args","passed"]},
                        "label":{"type":"literal","value":"fixed"}}},
                    "result":{"type":"json","part":1,"schema":schema(json!({"passed":{"type":"boolean"}}), &["passed"])}},
                "branch":{"type":"branch","condition":{"type":"boolean","value":{"type":"reference","path":["check","passed"]}}},
                "yes":{"type":"return","output":{"type":"reference","path":["check"]}},
                "no":{"type":"return","output":{"type":"reference","path":["check"]}}
            },
            "edges":[{"from":"check","to":"branch"},{"from":"branch","to":"yes","port":"true"},{"from":"branch","to":"no","port":"false"}]
        }
    })).unwrap()
}

fn context(
    plane: &WorkflowTestPlane,
    probe: Arc<Probe>,
    lifecycle: crate::agent::AttemptLifecycle,
) -> crate::runtime::subagent::AttemptSubagentContext {
    context_with_registration(
        plane,
        ToolRegistration::plain(definition(), probe),
        lifecycle,
    )
}

fn context_with_registration(
    plane: &WorkflowTestPlane,
    registration: ToolRegistration,
    lifecycle: crate::agent::AttemptLifecycle,
) -> crate::runtime::subagent::AttemptSubagentContext {
    context_with_workspace_policy(
        plane,
        registration,
        lifecycle,
        WorkspacePolicy::SharedWorkspace,
    )
}

fn context_with_workspace_policy(
    plane: &WorkflowTestPlane,
    registration: ToolRegistration,
    lifecycle: crate::agent::AttemptLifecycle,
    workspace_policy: WorkspacePolicy,
) -> crate::runtime::subagent::AttemptSubagentContext {
    let model_catalog = ModelCatalog::from_jsonc_slice(WORKFLOW_TEST_MODELS.as_bytes()).unwrap();
    let models = ModelBindingRegistry::new(
        model_catalog
            .resolve(&MapCredentialEnvironment::default())
            .unwrap(),
    )
    .unwrap();
    let model = ModelRef::parse("local/model").unwrap();
    let capability = Arc::new(CapabilitySnapshot::new(
        plane.conversation_id.clone(),
        plane.dir.path().join("workspace"),
        CapabilityRevision::new(1),
        Arc::new(ToolRegistry::new()), // deliberately inactive in the parent's model surface
        Arc::new(crate::capabilities::AvailableToolCatalog::new(vec![
            registration,
        ])),
        Arc::new(SkillSnapshot::new(Vec::new())),
        None,
        None,
        ToolEnvironment::new(),
        Arc::new(McpRuntimeLeaseAuthority::empty()),
        Arc::new(BTreeMap::new()),
    ));
    let agents = workflow_test_context_with_policy(
        plane,
        1,
        "Frozen candidate instructions",
        WorkflowCatalog::empty(),
        workspace_policy,
    );
    let resources = Arc::new(
        crate::runtime::RuntimeResourceSnapshot::new(
            RuntimeResourceRevision::new(1),
            Vec::new(),
            None,
            crate::context::ContextAssembly::new(),
            capability,
        )
        .with_subagent_catalog(agents.resources().subagents().clone())
        .with_subagent_admissions(BTreeSet::new(), BTreeSet::from([profile("reviewer")])),
    );
    let mut context = crate::runtime::subagent::AttemptSubagentContext::new(
        crate::runtime::identity::AttemptId::new("workflow-test-attempt"),
        resources,
        SessionModelConfig::of(model),
        models,
        ApprovalMode::Policy,
    );
    context.native = Some(Arc::new(NativeInvocationServices {
        lifecycle,
        runtime: crate::tools::runtime::ConversationToolRuntime::new(
            plane.conversation_id.clone(),
            plane.dir.path().join("workspace"),
            plane.dir.path().join("artifacts"),
        )
        .unwrap(),
        clock: Arc::new(crate::runtime::ManualMonotonicClock::new()),
        leaf_policy: crate::tools::deadline::ToolExecutionDeadlinePolicy::default(),
        turn: 1,
        scheduling: Arc::new(tokio::sync::RwLock::new(())),
        descendants: crate::tools::invocation::NativeChildScope::default(),
    }));
    context
}

#[tokio::test]
async fn tool_only_inactive_capability_has_no_provider_or_canonical_history_and_business_false_survives()
 {
    let plane = workflow_test_plane(1);
    let runtime = workflow_runtime(&plane);
    let probe = Probe::new(ToolExecutionStatus::Success);
    let context = context(
        &plane,
        probe.clone(),
        crate::agent::AttemptLifecycle::default(),
    );
    let frozen = context.resources().capability().clone();
    let (_, cancellation) = workflow_cancellation();
    let result = runtime
        .run_foreground(
            program(),
            ToolCallId::new("outer"),
            context,
            json!({"passed":false}),
            cancellation,
        )
        .await
        .unwrap();
    assert_eq!(result, json!({"passed":false}));
    assert!(frozen.tool_registry().model_definitions().is_empty());
    assert_eq!(probe.starts.load(Ordering::SeqCst), 1);
    let received = probe.received.lock().unwrap();
    assert!(matches!(received[0].id, ToolInvocationId::Workflow { .. }));
    assert_eq!(received[0].mode, ToolInvocationMode::Foreground);
    assert_eq!(
        received[0].arguments,
        json!({"passed":false,"label":"fixed"})
    );
    let events = plane.store.read_events(None, 128).unwrap().events;
    assert!(!events.iter().any(|event| matches!(
        event.event,
        RuntimeEvent::ModelRequestStarted { .. }
            | RuntimeEvent::AssistantMessageCommitted { .. }
            | RuntimeEvent::ToolMessageCommitted { .. }
    )));
    let native = events
        .iter()
        .filter_map(|event| match &event.event {
            RuntimeEvent::NativeToolInvocation { fact, .. } => Some(fact),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(matches!(
        native.last(),
        Some(NativeInvocationFact::Completed {
            status: ToolExecutionStatus::Success
        })
    ));
}

#[test]
fn native_failure_matrix_and_explicit_projection_commit_no_business_values() {
    let contract = WorkflowToolResult::Json {
        part: 0,
        schema: schema(json!({"passed":{"type":"boolean"}}), &["passed"]),
    };
    for status in [
        ToolExecutionStatus::Failed {
            error: "checker could not launch".into(),
        },
        ToolExecutionStatus::Denied {
            reason: "denied".into(),
        },
        ToolExecutionStatus::Cancelled {
            reason: CancellationReason::UserRequested,
            phase: ToolCancellationPhase::DuringExecution,
        },
        ToolExecutionStatus::TimedOut,
        ToolExecutionStatus::OutcomeUnknown {
            detail: "remote effect unconfirmed".into(),
        },
    ] {
        let error = contract
            .project(&terminal(status.clone()), &test_instance("check", "check"))
            .unwrap_err();
        assert_eq!(error.execution_status(), status);
        let parallel = WorkflowRunError::ParallelFailed {
            node: "fanout".into(),
            failures: BTreeMap::from([("leaf".into(), error)]),
        };
        assert_eq!(parallel.execution_status(), status);
    }
    let mut result = terminal(ToolExecutionStatus::Success);
    result.content = vec![ToolResultContent::Text(
        crate::message::content::TextBlock {
            text: "{\"passed\":true}".into(),
        },
    )];
    assert!(
        contract
            .project(&result, &test_instance("check", "check"))
            .is_err(),
        "JSON-looking logs are never parsed"
    );
    assert_eq!(
        WorkflowToolResult::Text { part: 0 }
            .project(&result, &test_instance("check", "check"))
            .unwrap(),
        json!("{\"passed\":true}")
    );
    result.content = vec![ToolResultContent::Json {
        value: json!({"passed": "yes"}),
    }];
    assert!(
        contract
            .project(&result, &test_instance("check", "check"))
            .is_err()
    );
}

#[tokio::test]
async fn cancellation_before_node_admission_starts_zero_executors() {
    let plane = workflow_test_plane(1);
    let runtime = workflow_runtime(&plane);
    let probe = Probe::new(ToolExecutionStatus::Success);
    let context = context(
        &plane,
        probe.clone(),
        crate::agent::AttemptLifecycle::default(),
    );
    let (trigger, cancellation) = workflow_cancellation();
    trigger.cancel(); // cancellation is observable before execute_block's node frontier
    assert!(
        runtime
            .run_foreground(
                program(),
                ToolCallId::new("outer"),
                context,
                json!({"passed":false}),
                cancellation
            )
            .await
            .is_err()
    );
    assert_eq!(probe.starts.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn cancellation_during_execution_drains_native_settlement_before_node_terminal() {
    let plane = workflow_test_plane(1);
    let runtime = workflow_runtime(&plane);
    let mut probe = Probe::new(ToolExecutionStatus::Success);
    Arc::get_mut(&mut probe).unwrap().release = Some(Arc::new(tokio::sync::Notify::new()));
    let gate = Arc::new(tokio::sync::Notify::new());
    Arc::get_mut(&mut probe).unwrap().settled = Some(gate.clone());
    let context = context(
        &plane,
        probe.clone(),
        crate::agent::AttemptLifecycle::default(),
    );
    let (trigger, cancellation) = workflow_cancellation();
    let mut started = probe.started.subscribe();
    let mut cancelled = probe.cancelled.subscribe();
    let task = tokio::spawn(async move {
        runtime
            .run_foreground(
                program(),
                ToolCallId::new("outer"),
                context,
                json!({"passed":false}),
                cancellation,
            )
            .await
    });
    started.wait_for(|value| *value == 1).await.unwrap();
    trigger.cancel();
    cancelled.wait_for(|value| *value).await.unwrap(); // executor is now parked in physical cleanup
    assert!(!task.is_finished());
    assert!(
        !plane
            .store
            .read_events(None, 128)
            .unwrap()
            .events
            .iter()
            .any(|event| matches!(event.event, RuntimeEvent::WorkflowNodeSettled { .. }))
    );
    gate.notify_one();
    let error = task.await.unwrap().unwrap_err();
    assert!(matches!(
        error.execution_status(),
        ToolExecutionStatus::Cancelled { .. }
    ));
    assert_eq!(probe.starts.load(Ordering::SeqCst), 1);
}

struct Ask;

#[tokio::test]
async fn native_observation_store_failure_has_no_permission_or_outcome_authority() {
    let plane = workflow_test_plane(1);
    let runtime = workflow_runtime(&plane);
    let probe = Probe::new(ToolExecutionStatus::Success);
    let context = context(
        &plane,
        probe.clone(),
        crate::agent::AttemptLifecycle::default(),
    );
    // Fail every observation, including Prepared, Started, Progress and Completed.
    // The store's real append boundary consumes these injected failures.
    plane.store.arm_fail_event_times(100);
    let (_, cancellation) = workflow_cancellation();
    let (result, _) = run_outer(runtime, program(), context, cancellation).await;
    assert_eq!(result.status, ToolExecutionStatus::Success);
    assert_eq!(probe.starts.load(Ordering::SeqCst), 1);
    assert!(
        plane
            .store
            .read_events(None, 128)
            .unwrap()
            .events
            .is_empty()
    );
}

#[tokio::test]
async fn durable_approval_preparation_failure_never_publishes_or_starts() {
    use crate::runtime::interaction::{InteractionCoordinator, RecordingInteractionAudit};
    let plane = workflow_test_plane(1);
    let runtime = workflow_runtime(&plane);
    let probe = Probe::new(ToolExecutionStatus::Success);
    let lifecycle = crate::runtime::types::ConversationLifecycle::new();
    assert!(lifecycle.activate());
    let audit = RecordingInteractionAudit::new(plane.conversation_id.clone());
    audit.fail_next_requested();
    let coordinator = Arc::new(InteractionCoordinator::new(
        plane.conversation_id.clone(),
        lifecycle,
        audit.clone(),
    ));
    coordinator.set_provider_available(true);
    let lifecycle = crate::agent::AttemptLifecycle::default()
        .with_pre_tool_policy(Arc::new(Ask))
        .with_native_interaction(coordinator.clone());
    let context = context(&plane, probe.clone(), lifecycle);
    let (_, cancellation) = workflow_cancellation();
    let error = runtime
        .run_foreground(
            program(),
            ToolCallId::new("outer"),
            context,
            json!({"passed":false}),
            cancellation,
        )
        .await
        .unwrap_err();
    assert!(matches!(error, WorkflowRunError::InvocationAuthority(_)));
    assert_eq!(probe.starts.load(Ordering::SeqCst), 0);
    assert_eq!(coordinator.pending_count(), 0);
    assert!(audit.events().is_empty());
}
async fn run_outer(
    runtime: WorkflowRuntime,
    program: Arc<WorkflowProgram>,
    context: crate::runtime::subagent::AttemptSubagentContext,
    cancellation: ExecutionCancellation,
) -> (
    ToolExecutionResult,
    Vec<crate::tools::invocation::InvocationFact>,
) {
    let executor = crate::tools::native::test_workflow_executor(runtime, program);
    drive_executor(executor, context, cancellation, None).await
}

async fn drive_executor(
    registration: (
        Arc<dyn ToolExecutor>,
        crate::tools::deadline::ForegroundPolicy,
    ),
    context: crate::runtime::subagent::AttemptSubagentContext,
    cancellation: ExecutionCancellation,
    completion_won: Option<Arc<dyn Fn() + Send + Sync>>,
) -> (
    ToolExecutionResult,
    Vec<crate::tools::invocation::InvocationFact>,
) {
    let (executor, policy) = registration;
    let services = context.native.as_ref().unwrap().clone();
    let progress = NativeProgressForTest;
    let native_context = ToolExecutionContext::new(
        services.runtime.conversation_id(),
        None,
        cancellation,
        services.runtime.workspace(),
        &progress,
        services.runtime.artifacts(),
        services.runtime.tool_output(),
        context.resources().capability().effective_environment(),
    );
    let native_context = native_context.with_subagent_context(context.clone());
    crate::tools::invocation::ForegroundInvocation {
        clock: &*services.clock,
        policy: services.leaf_policy,
        registration: policy,
        deadline_armed: None,
        cancellation_won: None,
        completion_won: completion_won.as_deref(),
    }
    .execute(
        &*executor,
        ToolInvocation {
            id: ToolInvocationId::Agent {
                call_id: ToolCallId::new("outer"),
            },
            tool_id: crate::runtime::identity::ToolId::new("tool-workflow-review"),
            tool_name: "review".into(),
            mode: ToolInvocationMode::Foreground,
            arguments: json!({"passed":false}),
        },
        crate::tools::deadline::ToolProgressCapability::None,
        native_context,
    )
    .await
}

struct NativeProgressForTest;
impl crate::tools::executor::ProgressReporter for NativeProgressForTest {
    fn report(&self, _: ToolProgress) {}
}

#[tokio::test]
async fn outer_total_deadline_survives_new_leaf_at_capacity_one_and_drains_settlement() {
    let plane = workflow_test_plane(1);
    let runtime = workflow_runtime(&plane);
    let mut probe = Probe::new(ToolExecutionStatus::Success);
    let release = Arc::new(tokio::sync::Notify::new());
    let settle = Arc::new(tokio::sync::Notify::new());
    Arc::get_mut(&mut probe).unwrap().release = Some(release.clone());
    Arc::get_mut(&mut probe).unwrap().settled = Some(settle.clone());
    let mut context = context(
        &plane,
        probe.clone(),
        crate::agent::AttemptLifecycle::default(),
    );
    let clock = Arc::new(crate::runtime::ManualMonotonicClock::new());
    let services = Arc::make_mut(context.native.as_mut().unwrap());
    services.clock = clock.clone();
    services.leaf_policy = crate::tools::deadline::ToolExecutionDeadlinePolicy::new(
        std::time::Duration::from_secs(1),
        None,
    );
    let mut definition = program_definition();
    definition
        .block
        .nodes
        .insert("second".into(), definition.block.nodes["check"].clone());
    definition.block.edges.retain(|edge| edge.from != "check");
    definition
        .block
        .edges
        .extend([edge("check", "second"), edge("second", "branch")]);
    let program = Arc::new(compile_test(definition).unwrap());
    let mut started = probe.started.subscribe();
    let mut cancelled = probe.cancelled.subscribe();
    let (_, cancellation) = workflow_cancellation();
    let task = tokio::spawn(run_outer(runtime, program, context, cancellation));
    started.wait_for(|n| *n == 1).await.unwrap(); // outer and first leaf armed at t=0
    clock.advance(60);
    release.notify_one();
    started.wait_for(|n| *n == 2).await.unwrap(); // first sequential leaf released its exclusive gate
    clock.advance(40); // outer t=100; second leaf only 40ms into its own 1000ms policy
    cancelled.wait_for(|value| *value).await.unwrap();
    assert!(
        !task.is_finished(),
        "outer must await the owned physical settlement"
    );
    settle.notify_one();
    let (result, facts) = task.await.unwrap();
    assert_eq!(result.status, ToolExecutionStatus::TimedOut);
    assert!(facts.iter().any(|fact| matches!(
        fact,
        crate::tools::invocation::InvocationFact::Deadline {
            kind: crate::tools::deadline::ToolDeadlineKind::Hard
        }
    )));
    let events = plane.store.read_events(None, 256).unwrap().events;
    let completed = events
        .iter()
        .filter_map(|event| match &event.event {
            RuntimeEvent::NativeToolInvocation {
                fact: NativeInvocationFact::Completed { status },
                ..
            } => Some(status),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(completed.len(), 2);
    assert_eq!(
        *completed[0],
        ToolExecutionStatus::Success,
        "settled child is immutable"
    );
    assert_eq!(*completed[1], ToolExecutionStatus::TimedOut);
    let causes =
        events
            .iter()
            .filter_map(|event| match &event.event {
                RuntimeEvent::NativeToolInvocation {
                    fact:
                        NativeInvocationFact::Lifecycle {
                            fact:
                                crate::tools::invocation::InvocationFact::CancellationRequested {
                                    cause,
                                },
                        },
                    ..
                } => Some(*cause),
                _ => None,
            })
            .collect::<Vec<_>>();
    assert_eq!(
        causes,
        vec![crate::tools::deadline::ToolCancellationCause::Deadline(
            crate::tools::deadline::ToolDeadlineKind::Hard
        )]
    );
}

#[tokio::test]
async fn agent_tool_branch_return_uses_committed_typed_binding() {
    let plane = workflow_test_plane(1);
    let mut child = stage_workflow_child(&plane);
    let runtime = workflow_runtime(&plane);
    let probe = Probe::new(ToolExecutionStatus::Success);
    let context = context(
        &plane,
        probe.clone(),
        crate::agent::AttemptLifecycle::default(),
    );
    let mut definition = program_definition();
    definition.block.entry = "agent".into();
    definition.block.nodes.insert(
        "agent".into(),
        WorkflowNodeDefinition::Agent {
            profile: profile("reviewer"),
            task: "Return machine-compatible findings".into(),
            input: BTreeMap::new(),
            output: schema(json!({"passed":{"type":"boolean"}}), &["passed"]),
        },
    );
    if let WorkflowNodeDefinition::Tool {
        arguments: WorkflowValue::Object { fields },
        ..
    } = definition.block.nodes.get_mut("check").unwrap()
    {
        fields.insert(
            "passed".into(),
            WorkflowValue::Reference {
                path: vec!["agent".into(), "passed".into()],
            },
        );
    }
    definition.block.edges.push(edge("agent", "check"));
    let program = Arc::new(compile_test(definition).unwrap());
    let (_, cancellation) = workflow_cancellation();
    let task = tokio::spawn(async move {
        runtime
            .run_foreground(
                program,
                ToolCallId::new("outer"),
                context,
                json!({"passed":true}),
                cancellation,
            )
            .await
    });
    child.expect_delegate().await; // exact Agent admission; Tool has not started
    assert_eq!(probe.starts.load(Ordering::SeqCst), 0);
    child
        .send_result(
            crate::runtime::subagent::ipc::ChildResultStatus::Succeeded,
            Some(r#"{"passed":false}"#),
        )
        .await;
    assert_eq!(task.await.unwrap().unwrap(), json!({"passed":false}));
    assert_eq!(
        probe.received.lock().unwrap()[0].arguments,
        json!({"passed":false,"label":"fixed"})
    );
}

impl crate::agent::PreToolPolicy for Ask {
    fn evaluate<'a>(
        &'a self,
        view: &'a crate::agent::PreToolView<'a>,
    ) -> futures_util::future::BoxFuture<
        'a,
        Result<crate::agent::PreToolDecision, crate::agent::LifecycleError>,
    > {
        assert_eq!(view.arguments, &json!({"passed":false,"label":"fixed"}));
        Box::pin(async {
            Ok(crate::agent::PreToolDecision::Ask {
                reason: "approval required".into(),
            })
        })
    }
}

struct Answer {
    allow: bool,
    requests: AtomicUsize,
}
impl crate::runtime::interaction::TestInteractionRendezvous for Answer {
    fn request_approval(
        &self,
        facts: crate::runtime::interaction::ApprovalFacts,
        _: ExecutionCancellation,
    ) -> futures_util::future::BoxFuture<'_, crate::runtime::interaction::InteractionOutcome> {
        use crate::runtime::interaction::{
            ApprovalDecision, InteractionOutcome, InteractionResponse,
        };
        assert!(matches!(
            facts.invocation_id,
            ToolInvocationId::Workflow { .. }
        ));
        assert_eq!(facts.arguments, json!({"passed":false,"label":"fixed"}));
        self.requests.fetch_add(1, Ordering::SeqCst);
        let decision = if self.allow {
            ApprovalDecision::Allow
        } else {
            ApprovalDecision::Deny {
                reason: "no".into(),
            }
        };
        Box::pin(async move {
            InteractionOutcome::Responded {
                response: InteractionResponse::Approval { decision },
            }
        })
    }
}

#[tokio::test]
async fn ordinary_approval_denial_starts_zero_and_allow_executes_exact_prepared_invocation_once() {
    for allow in [false, true] {
        let plane = workflow_test_plane(1);
        let runtime = workflow_runtime(&plane);
        let probe = Probe::new(ToolExecutionStatus::Success);
        let answer = Arc::new(Answer {
            allow,
            requests: AtomicUsize::new(0),
        });
        let lifecycle = crate::agent::AttemptLifecycle::default()
            .with_pre_tool_policy(Arc::new(Ask))
            .with_test_interaction_rendezvous(answer.clone());
        let mut registration = ToolRegistration::plain(definition(), probe.clone());
        registration.normalizer = |arguments| {
            let mut prepared = arguments.clone();
            prepared["passed"] = json!(false);
            Ok(prepared)
        };
        let context = context_with_registration(&plane, registration, lifecycle);
        let (_, cancellation) = workflow_cancellation();
        let result = runtime
            .run_foreground(
                program(),
                ToolCallId::new("outer"),
                context,
                json!({"passed":true}), // approval must see normalized false, not this caller input
                cancellation,
            )
            .await;
        assert_eq!(answer.requests.load(Ordering::SeqCst), 1);
        assert_eq!(probe.starts.load(Ordering::SeqCst), usize::from(allow));
        if allow {
            assert_eq!(result.unwrap(), json!({"passed":false}));
        } else {
            assert!(matches!(
                result.unwrap_err().execution_status(),
                ToolExecutionStatus::Denied { .. }
            ));
        }
    }
}

#[tokio::test]
async fn normalization_and_schema_rejection_start_zero_executors() {
    for normalize in [false, true] {
        let plane = workflow_test_plane(1);
        let runtime = workflow_runtime(&plane);
        let probe = Probe::new(ToolExecutionStatus::Success);
        let mut registration = ToolRegistration::plain(definition(), probe.clone());
        if normalize {
            registration.normalizer = |_| Err("normalization rejected".into());
        } else {
            registration.definition.input_schema =
                schema(json!({"missing":{"type":"string"}}), &["missing"]);
        }
        let context = context_with_registration(
            &plane,
            registration,
            crate::agent::AttemptLifecycle::default(),
        );
        let (_, cancellation) = workflow_cancellation();
        let error = runtime
            .run_foreground(
                program(),
                ToolCallId::new("outer"),
                context,
                json!({"passed":false}),
                cancellation,
            )
            .await
            .unwrap_err();
        assert!(matches!(
            error.execution_status(),
            ToolExecutionStatus::Failed { .. }
        ));
        assert_eq!(probe.starts.load(Ordering::SeqCst), 0);
    }
}

#[test]
fn authority_rejects_changed_identity_and_unadmitted_selection() {
    use crate::capabilities::selection::ToolSelector;
    let probe = Probe::new(ToolExecutionStatus::Success);
    let registration = ToolRegistration::plain(definition(), probe);
    let mut changed = definition();
    changed.description = "replacement".into();
    assert!(
        registration
            .prepare_fixed(
                ToolInvocationId::Workflow {
                    node: Box::new(test_instance("check", "check"))
                },
                &changed,
                &json!({})
            )
            .is_err()
    );
    let program = program();
    let mut definition = WorkflowDefinition {
        workspace: None,
        description: "unadmitted".into(),
        tools: BTreeSet::new(),
        timeout_ms: 100,
        block: WorkflowBlock {
            input: json!({"type":"object"}),
            output: json!({"type":"object"}),
            entry: "tool".into(),
            nodes: BTreeMap::from([
                (
                    "tool".into(),
                    WorkflowNodeDefinition::Tool {
                        selector: ToolSelector::Builtin {
                            name: "check".into(),
                        },
                        arguments: WorkflowValue::Literal { value: json!({}) },
                        result: WorkflowToolResult::Json {
                            part: 0,
                            schema: json!({"type":"object"}),
                        },
                    },
                ),
                (
                    "done".into(),
                    WorkflowNodeDefinition::Return {
                        output: WorkflowValue::Literal { value: json!({}) },
                    },
                ),
            ]),
            edges: vec![edge("tool", "done")],
        },
    };
    assert!(compile_test(definition.clone()).is_err());
    definition.tools = program.tools.clone();
    assert!(compile_test(definition).is_ok());
}
