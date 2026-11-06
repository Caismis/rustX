//! Real scheduled Tool dispatch and SQLite ownership with controlled response loss.
#![allow(clippy::too_many_lines)]
use super::*;
use crate::durable::{ScheduleStore, ScheduledWorkflowStore};
use crate::runtime::workflow::scheduled::lifecycle::*;
use crate::scheduler::*;
use tokio::sync::{Mutex, oneshot, watch};

struct LostResponse {
    response: Mutex<Option<oneshot::Receiver<()>>>,
    started: watch::Sender<bool>,
    cancelled: watch::Sender<bool>,
}
impl ToolExecutor for LostResponse {
    fn start<'a>(
        &'a self,
        _: ToolInvocation,
        context: ToolExecutionContext<'a>,
    ) -> ToolExecutionHandle<'a> {
        let cancellation = context.cancellation;
        ToolExecutionHandle::new(
            Box::pin(async move {
                self.started.send_replace(true);
                let _ = self.response.lock().await.take().unwrap().await;
                crate::tools::invocation::terminal(ToolExecutionStatus::OutcomeUnknown {
                    detail: "dispatched remote response channel closed".into(),
                })
            }),
            Box::pin(async move {
                cancellation.cancelled().await;
                self.cancelled.send_replace(true);
                std::future::pending().await
            }),
        )
    }
    fn progress_capability(&self) -> crate::tools::deadline::ToolProgressCapability {
        crate::tools::deadline::ToolProgressCapability::None
    }
}
struct WallClock(chrono::DateTime<chrono::Utc>);
impl crate::runtime::types::RuntimeClock for WallClock {
    fn now(&self) -> chrono::DateTime<chrono::Utc> {
        self.0
    }
}

fn native_context(
    current: &ScheduledWorkflowPreparation,
    start: &ScheduledWorkflowHead,
    clock: Arc<crate::runtime::ManualMonotonicClock>,
) -> crate::runtime::subagent::NativeExecutionContext {
    let snapshot = current.resources.capability();
    let capability = crate::capabilities::CapabilityCoordinator::new(
        crate::capabilities::CapabilityCoordinatorConfig {
            conversation_id: current.binding.conversation_id.clone(),
            workspace: crate::tools::workspace::Workspace::new(snapshot.workspace_root()).unwrap(),
            environment_store_root: snapshot
                .workspace_root()
                .parent()
                .unwrap()
                .join("environments"),
            source_demand: crate::capabilities::source::ToolSourceDemand::default(),
            base_tool_registry: Arc::new(
                ToolRegistry::from_registrations(
                    snapshot.available_tools().registrations().to_vec(),
                )
                .unwrap(),
            ),
            extension_tools: crate::extensions::ExtensionToolPlane::none(),
            agent_activation: crate::capabilities::AgentActivation::default(),
            skill_discovery: crate::skills::SkillDiscoveryConfig::default(),
            mcp_servers: snapshot.mcp_servers().clone(),
            base_environment: snapshot.effective_environment().clone(),
        },
    )
    .unwrap();
    let mut context = current.execution_context(capability, start.id.clone());
    context.native = Some(Arc::new(
        crate::tools::invocation::NativeInvocationServices {
            lifecycle: crate::agent::AttemptLifecycle::default(),
            runtime: crate::tools::runtime::ConversationToolRuntime::new(
                current.binding.conversation_id.clone(),
                snapshot.workspace_root(),
                snapshot
                    .workspace_root()
                    .parent()
                    .unwrap()
                    .join("artifacts"),
            )
            .unwrap(),
            clock,
            leaf_policy: current.policy.tool_deadline,
            execution: crate::runtime::interaction::InteractionExecution::Scheduled {
                run: start.run_id.clone(),
            },
            scheduling: Arc::new(tokio::sync::RwLock::new(())),
            descendants: crate::tools::invocation::NativeChildScope::default(),
        },
    ));
    context
}

async fn assert_uncertain_capacity(guard_expiry: bool) {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let (response, receiver) = oneshot::channel();
    let probe = Arc::new(LostResponse {
        response: Mutex::new(Some(receiver)),
        started: watch::channel(false).0,
        cancelled: watch::channel(false).0,
    });
    let current = fixture_with_executor(
        &workspace,
        &Inputs {
            kind: NodeKind::Tool,
            ..Inputs::default()
        },
        probe.clone(),
    );
    let store = Arc::new(
        crate::durable::SqliteConversationStore::in_memory(current.binding.conversation_id.clone())
            .unwrap(),
    );
    let now = WallClock(
        chrono::DateTime::parse_from_rfc3339("2026-10-24T06:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc),
    );
    let prepared = current.prepare(&id(), &args()).unwrap();
    let ScheduleCommandResult::Applied { receipt } = store
        .schedule_workflow_command(
            &current.binding,
            &ScheduleCommand {
                command_id: "uncertain-tool".into(),
                mutation: ScheduleMutation::CreateWorkflow {
                    title: "Retain physical ownership".into(),
                    workflow_id: id(),
                    arguments: args(),
                    timing: ScheduleTiming::Interval { every_seconds: 60 },
                },
            },
            &now,
            &ScheduleCommandAuthority::RuntimeControl,
            prepared.contract(),
        )
        .unwrap()
    else {
        panic!("create")
    };
    let due = WallClock(now.0 + chrono::Duration::seconds(60));
    let ScheduleAdmission::WorkflowAdmitted { start, .. } = store
        .admit_scheduled_target(&receipt.id, receipt.revision, &due)
        .unwrap()
    else {
        panic!("admit")
    };
    let claim = store
        .scheduled_workflow_claim(&start.id, prepared.contract(), "native-test-owner", due.0)
        .unwrap()
        .unwrap();
    let clock = Arc::new(crate::runtime::ManualMonotonicClock::new());
    let context = native_context(&current, &start, clock.clone());
    let registry = crate::runtime::subagent::SubagentRegistry::new(
        crate::runtime::subagent::SubagentRegistryConfig {
            conversation_id: current.binding.conversation_id.clone(),
            agent_id: crate::runtime::identity::AgentId::new("scheduled-parent"),
            mailbox: crate::runtime::inbound::ConversationInboundMailbox::over_store(store.clone()),
            clock: Arc::new(crate::runtime::types::SystemClock),
            monotonic_clock: clock.clone(),
            spawn: crate::runtime::subagent::SubagentSpawnPlan {
                session_id: current.binding.session_id.clone(),
                program: "/not-invoked/rustx".into(),
                product_root: crate::runtime::local_storage::ProductRoot::create(
                    &root.path().join("children"),
                )
                .unwrap(),
            },
            workspace,
            max_active: 1,
        },
    );
    let runtime = crate::runtime::workflow::WorkflowRuntime::new(
        registry,
        store.clone(),
        crate::runtime::workflow::read_model::WorkflowReadModel::new(
            current.binding.conversation_id.clone(),
        ),
    );
    let signal = crate::runtime::cancellation::CancellationSignal::new();
    let cancellation = crate::runtime::cancellation::ExecutionCancellation::detached(
        signal.clone(),
        crate::runtime::types::CancellationReason::UserRequested,
    );
    let mut started = probe.started.subscribe();
    let mut cancelled = probe.cancelled.subscribe();
    let task = tokio::spawn(async move {
        runtime
            .run_scheduled(claim, prepared, context, cancellation)
            .await
    });
    started.wait_for(|started| *started).await.unwrap();
    if guard_expiry {
        signal.cancel();
        cancelled.wait_for(|cancelled| *cancelled).await.unwrap();
        clock.advance(
            crate::tools::deadline::TOOL_SETTLEMENT_CONTROL_GUARD
                .as_millis()
                .try_into()
                .unwrap(),
        );
    } else {
        drop(response);
    }
    let result = task.await.unwrap().unwrap_err();
    assert!(matches!(
        result.execution_status(),
        ToolExecutionStatus::OutcomeUnknown { .. }
    ));
    let recovery = store
        .scheduled_workflow_recovery(&start.id, 0, 100)
        .unwrap()
        .unwrap();
    assert_eq!(recovery.effects.len(), 1);
    assert!(
        !recovery.effects[0].physical_settlement_proven,
        "a dispatched unknown Tool retains its physical ownership"
    );
    assert_eq!(
        store
            .scheduled_workflow_settle(
                &start.id,
                ScheduledWorkflowOutcome::OutcomeUnknown {
                    reason: "remote physical termination is unproven".into()
                },
                ScheduledWorkflowQuiescence {
                    source: QuiescenceSource::NativeExecutor
                },
                None,
                due.0
            )
            .unwrap(),
        ScheduledWorkflowSettlement::AwaitingNativeProof
    );
    let later = WallClock(due.0 + chrono::Duration::seconds(60));
    assert!(matches!(
        store
            .admit_scheduled_target(&receipt.id, 2, &later)
            .unwrap(),
        ScheduleAdmission::WaitingForTarget
    ));
    store
        .schedule_command(
            &current.binding,
            &ScheduleCommand {
                command_id: "disable-future".into(),
                mutation: ScheduleMutation::SetEnabled {
                    id: receipt.id,
                    expected_revision: 2,
                    enabled: false,
                },
            },
            &later,
            &ScheduleCommandAuthority::RuntimeControl,
        )
        .unwrap();
    assert_eq!(store.schedule_summary().unwrap().enabled, 0);
    let recovery = store
        .scheduled_workflow_recovery(&start.id, 0, 100)
        .unwrap()
        .unwrap();
    assert!(recovery.capacity_blocked && recovery.deletion_blocked);
    assert!(!recovery.start.native_quiescence_proven);
}

#[tokio::test]
async fn scheduler85_review_unknown_completion_retains_native_capacity_and_deletion_blocker() {
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        assert_uncertain_capacity(false),
    )
    .await
    .unwrap();
}
#[tokio::test]
async fn scheduler85_review_settlement_guard_expiry_retains_native_capacity_and_deletion_blocker() {
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        assert_uncertain_capacity(true),
    )
    .await
    .unwrap();
}
