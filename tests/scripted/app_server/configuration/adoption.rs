//! Adoption of prepared configuration by live Sessions: admitted attempts keep
//! their capture, candidates conflict and supersede, and the admission gate
//! orders busy adoption without cancelling work.
#![allow(clippy::large_futures)] // bounded fixture futures; no recursive or unbounded stack growth
use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue383_disabled_extension_keeps_admitted_configuration_until_settlement() {
    Box::pin(bounded(async {
        let fixture = Fixture::with_session_count(Some("read"), 1).await;
        let id = &fixture.sessions[0].id;
        fixture.manager.load(id, None).await.unwrap();
        write(
            &fixture,
            0,
            ConfigMutation::Todo {
                authored: Some(crate::extensions::TodoExtensionDocument { enabled: true }),
            },
        )
        .await;
        let enabled = settled(&fixture, 0).await;
        if let Some(candidate) = enabled.candidate {
            fixture
                .manager
                .adopt_configuration(id, &candidate.identity, candidate.expected_binding)
                .unwrap();
        }
        let runtime = fixture.manager.configuration_runtime(id).unwrap();
        runtime.submit_inbound(input("request-A")).unwrap();
        fixture.gates[0].wait_entered().await;
        let admitted = runtime
            .configuration_view()
            .unwrap()
            .admitted_attempt
            .unwrap();
        write(
            &fixture,
            0,
            ConfigMutation::Todo {
                authored: Some(crate::extensions::TodoExtensionDocument { enabled: false }),
            },
        )
        .await;
        let disabled = settled(&fixture, 0).await;
        assert_eq!(
            runtime
                .configuration_view()
                .unwrap()
                .admitted_attempt
                .unwrap(),
            admitted
        );
        let mut settlement = SettlementReceiptWait::capture(&runtime);
        fixture.gates[0].release();
        settlement.wait().await;
        runtime.wait_for_configuration_admissions().await;
        let requests = fixture.provider.request_bodies();
        assert_eq!(requests.len(), 2);
        for body in &requests {
            let request: serde_json::Value = serde_json::from_str(body).unwrap();
            assert!(
                request["tools"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|tool| tool["function"]["name"] == "todo"),
                "old admitted steps retain Todo"
            );
        }
        if let Some(candidate) = disabled.candidate {
            fixture
                .manager
                .adopt_configuration(id, &candidate.identity, candidate.expected_binding)
                .unwrap();
        }
        runtime.submit_inbound(input("request-B")).unwrap();
        fixture.gates[1].wait_entered().await;
        let future: serde_json::Value =
            serde_json::from_str(fixture.provider.request_bodies().last().unwrap()).unwrap();
        assert!(
            !future["tools"]
                .as_array()
                .unwrap()
                .iter()
                .any(|tool| tool["function"]["name"] == "todo")
        );
        let mut settlement = SettlementReceiptWait::capture(&runtime);
        fixture.gates[1].release();
        settlement.wait().await;
        fixture.close().await;
    }))
    .await;
}

#[tokio::test]
async fn t04_session_relative_prefix_and_t09_policy_noop() {
    let fixture = Fixture::new().await;
    for session in &fixture.sessions {
        fixture.manager.load(&session.id, None).await.unwrap();
    }
    write(
        &fixture,
        0,
        ConfigMutation::Instructions {
            authored: Some("P2".into()),
        },
    )
    .await;
    let s1 = settled(&fixture, 0).await;
    let s2 = settled(&fixture, 1).await;
    assert!(s1.candidate.is_some(), "{s1:?}");
    let candidate = s2.candidate.unwrap();
    fixture
        .manager
        .adopt_configuration(
            &fixture.sessions[1].id,
            &candidate.identity,
            candidate.expected_binding,
        )
        .unwrap();
    let runtimes: Vec<_> = fixture
        .sessions
        .iter()
        .map(|s| fixture.manager.configuration_runtime(&s.id).unwrap())
        .collect();
    let p1 = runtimes[0]
        .runtime_resources()
        .configuration()
        .unwrap()
        .config
        .agent
        .instructions
        .clone();
    assert_ne!(p1, "P2");
    assert_eq!(
        runtimes[1]
            .runtime_resources()
            .configuration()
            .unwrap()
            .config
            .agent
            .instructions,
        "P2"
    );
    write(
        &fixture,
        0,
        ConfigMutation::Approval {
            authored: Some(crate::runtime::ApprovalMode::FullAccess),
        },
    )
    .await;
    settled(&fixture, 0).await;
    settled(&fixture, 1).await;
    assert_eq!(
        runtimes[0]
            .runtime_resources()
            .configuration()
            .unwrap()
            .config
            .agent
            .instructions,
        p1
    );
    assert_eq!(
        runtimes[1]
            .runtime_resources()
            .configuration()
            .unwrap()
            .config
            .agent
            .instructions,
        "P2"
    );
    for runtime in &runtimes {
        assert_eq!(
            runtime.configuration_view().unwrap().approval_mode,
            crate::runtime::ApprovalMode::FullAccess
        );
    }
    let revision = runtimes[1].runtime_resources().revision();
    fixture
        .manager
        .reconcile_configuration(
            &crate::local_runtime::configuration::settings::SourceTarget::Workspace {
                directory: fixture.workspaces[1].clone(),
            },
        )
        .await
        .unwrap();
    let healthy = settled(&fixture, 1).await;
    assert!(healthy.candidate.is_none());
    assert_eq!(runtimes[1].runtime_resources().revision(), revision);
    fixture.manager.drain_all_runtimes().await;
}

#[tokio::test]
async fn t11_concrete_candidate_conflict_and_t09_healthy_rescan_preserves_candidate() {
    let fixture = Fixture::with_session_count(None, 1).await;
    let id = &fixture.sessions[0].id;
    fixture.manager.load(id, None).await.unwrap();
    write(
        &fixture,
        0,
        ConfigMutation::Instructions {
            authored: Some("candidate one".into()),
        },
    )
    .await;
    let inspected = settled(&fixture, 0).await;
    let old = inspected.candidate.clone().unwrap();
    let probe = fixture
        .manager
        .probe(&fixture.sessions[0].active_conversation_id);
    let preparations = probe.configuration_preparations.load(Ordering::SeqCst);
    fixture
        .manager
        .reconcile_configuration(
            &crate::local_runtime::configuration::settings::SourceTarget::Workspace {
                directory: fixture.workspaces[0].clone(),
            },
        )
        .await
        .unwrap();
    assert_eq!(settled(&fixture, 0).await, inspected);
    assert_eq!(
        probe.configuration_preparations.load(Ordering::SeqCst),
        preparations
    );
    write(
        &fixture,
        0,
        ConfigMutation::Instructions {
            authored: Some("candidate two".into()),
        },
    )
    .await;
    let newer = settled(&fixture, 0).await.candidate.unwrap();
    assert_ne!(old.identity.input_revision, newer.identity.input_revision);
    assert_ne!(old.identity.attempt, newer.identity.attempt);
    assert_eq!(
        fixture
            .manager
            .adopt_configuration(id, &old.identity, old.expected_binding),
        Err(AdoptionError::Conflict)
    );
    assert_eq!(
        fixture
            .manager
            .adopt_configuration(id, &newer.identity, newer.expected_binding + 1),
        Err(AdoptionError::Conflict)
    );
    let applied = fixture
        .manager
        .adopt_configuration(id, &newer.identity, newer.expected_binding)
        .unwrap();
    assert_eq!(
        applied.units[&ApplyUnit::Instructions],
        UnitApplication::Applied
    );
    assert!(applied.candidate.is_none());
    fixture.manager.drain_all_runtimes().await;
}

#[tokio::test]
async fn t15_unload_load_does_not_adopt_pending_context() {
    let fixture = Fixture::with_session_count(None, 1).await;
    let id = &fixture.sessions[0].id;
    let loaded = fixture.manager.load(id, None).await.unwrap();
    let before = fixture
        .manager
        .configuration_runtime(id)
        .unwrap()
        .runtime_resources();
    write(
        &fixture,
        0,
        ConfigMutation::Instructions {
            authored: Some("awaiting adoption".into()),
        },
    )
    .await;
    let original_candidate = settled(&fixture, 0).await.candidate.unwrap();
    fixture.manager.unload(&loaded.conversation).await.unwrap();
    fixture.manager.load(id, None).await.unwrap();
    assert_eq!(
        fixture
            .manager
            .configuration_runtime(id)
            .unwrap()
            .runtime_resources()
            .configuration()
            .unwrap()
            .config
            .agent
            .instructions,
        before.configuration().unwrap().config.agent.instructions
    );
    let rebound = settled(&fixture, 0).await.candidate.unwrap();
    assert_eq!(
        rebound.identity.input_revision,
        original_candidate.identity.input_revision
    );
    assert!(rebound.identity.attempt > original_candidate.identity.attempt);
    fixture
        .manager
        .adopt_configuration(id, &rebound.identity, rebound.expected_binding)
        .unwrap();
    assert!(
        fixture.provider.request_bodies().is_empty(),
        "adoption invokes no model"
    );
    assert_eq!(
        fixture
            .manager
            .configuration_runtime(id)
            .unwrap()
            .runtime_resources()
            .configuration()
            .unwrap()
            .config
            .agent
            .instructions,
        "awaiting adoption"
    );
    fixture.manager.drain_all_runtimes().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t01_new_attempt_captures_automatic_policy_old_attempt_retains_capture() {
    bounded(async {
        let fixture = Fixture::with_session_count(None, 1).await;
        let id = &fixture.sessions[0].id;
        fixture.manager.load(id, None).await.unwrap();
        let runtime = fixture.manager.configuration_runtime(id).unwrap();
        runtime.submit_inbound(input("request-A")).unwrap();
        fixture.gates[0].wait_entered().await;
        let captured = runtime
            .configuration_view()
            .unwrap()
            .admitted_attempt
            .unwrap();
        write(
            &fixture,
            0,
            ConfigMutation::Approval {
                authored: Some(crate::runtime::ApprovalMode::FullAccess),
            },
        )
        .await;
        write(
            &fixture,
            0,
            ConfigMutation::ModelTimeout {
                authored: Some(crate::local_runtime::authoring::TimeoutLayer {
                    response_start_timeout_ms: Some(41_000),
                    stream_idle_timeout_ms: Some(42_000),
                }),
            },
        )
        .await;
        let applied = settled(&fixture, 0).await;
        assert_eq!(
            applied.units[&ApplyUnit::ExecutionPolicy],
            UnitApplication::Applied
        );
        assert!(applied.candidate.is_none());
        assert_eq!(
            runtime.configuration_view().unwrap().approval_mode,
            crate::runtime::ApprovalMode::FullAccess
        );
        assert_eq!(
            runtime
                .configuration_view()
                .unwrap()
                .admitted_attempt
                .unwrap(),
            captured
        );
        let mut settlement = SettlementReceiptWait::capture(&runtime);
        fixture.gates[0].release();
        settlement.wait().await;
        runtime.submit_inbound(input("request-B")).unwrap();
        fixture.gates[1].wait_entered().await;
        let later = runtime
            .configuration_view()
            .unwrap()
            .admitted_attempt
            .unwrap();
        assert_eq!(
            later.approval_mode,
            crate::runtime::ApprovalMode::FullAccess
        );
        assert!(later.generation > captured.generation);
        assert_ne!(later.model_timeout, captured.model_timeout);
        fixture.gates[1].release();
        settlement.wait().await;
        fixture.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t06_offside_preparation_allows_admission_and_t07_new_failure_supersedes_old_candidate() {
    bounded(async {
        let fixture = Fixture::with_session_count(None, 1).await;
        let id = &fixture.sessions[0].id;
        fixture.manager.load(id, None).await.unwrap();
        let runtime = fixture.manager.configuration_runtime(id).unwrap();
        let before = runtime.runtime_resources();
        let probe = fixture
            .manager
            .probe(&fixture.sessions[0].active_conversation_id);
        probe.before_configuration_prepare.arm();
        write(
            &fixture,
            0,
            ConfigMutation::Instructions {
                authored: Some("superseded candidate".into()),
            },
        )
        .await;
        probe.before_configuration_prepare.entered().await;
        let old = fixture
            .manager
            .configuration_application(id)
            .unwrap()
            .desired;
        runtime.submit_inbound(input("request-A")).unwrap();
        fixture.gates[0].wait_entered().await;
        assert!(Arc::ptr_eq(&before, &runtime.runtime_resources()));
        // External source ingestion, including its failure, wins the source
        // fence while old preparation is parked off-side.
        std::fs::write(
            fixture.workspaces[0].join("rustx.toml"),
            "[agent]\nworkflows=['missing']\n",
        )
        .unwrap();
        fixture
            .manager
            .reconcile_configuration(
                &crate::local_runtime::configuration::settings::SourceTarget::Workspace {
                    directory: fixture.workspaces[0].clone(),
                },
            )
            .await
            .unwrap();
        probe.before_configuration_prepare.release();
        let latest = settled(&fixture, 0).await;
        assert_ne!(latest.desired, old);
        assert!(
            latest
                .units
                .values()
                .any(|unit| matches!(unit, UnitApplication::Failed { .. })),
            "{latest:?}"
        );
        // Independent instructions remain valid, but only a newly prepared
        // candidate under the newest identity can be offered.
        let candidate = latest.candidate.unwrap();
        assert_eq!(candidate.identity, latest.desired);
        assert_ne!(candidate.identity, old);
        assert!(matches!(
            latest.units[&ApplyUnit::Capabilities],
            UnitApplication::Failed { .. }
        ));
        assert!(Arc::ptr_eq(&before, &runtime.runtime_resources()));
        // Armed before the trigger: settlement wakes only registered waiters.
        let settled = runtime.settlement_signal().notified();
        tokio::pin!(settled);
        settled.as_mut().enable();
        fixture.gates[0].release();
        settled.await;
        fixture.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t11_admission_gate_orders_busy_adoption_without_cancelling_attempt() {
    bounded(async {
        let fixture = Fixture::with_session_count(None, 1).await;
        let id = fixture.sessions[0].id.clone();
        fixture.manager.load(&id, None).await.unwrap();
        let runtime = fixture.manager.configuration_runtime(&id).unwrap();
        write(
            &fixture,
            0,
            ConfigMutation::Instructions {
                authored: Some("adopt when idle".into()),
            },
        )
        .await;
        let candidate = settled(&fixture, 0).await.candidate.unwrap();
        let mut eligibility = EligibilityStream::open(&fixture, &id);
        eligibility.until(AdoptionEligibility::Eligible).await;
        let gate = Arc::new(crate::runtime::conversation_runtime::Gate::default());
        let release = gate.arm_scoped();
        runtime.install_configuration_admission_gate(gate.clone());
        runtime.submit_inbound(input("request-A")).unwrap();
        tokio::task::spawn_blocking(move || gate.wait_entered())
            .await
            .unwrap();
        let manager = fixture.manager.clone();
        let adopting_id = id.clone();
        let adopting_candidate = candidate.clone();
        let (arrived, arrival) = tokio::sync::oneshot::channel();
        let adoption = tokio::task::spawn_blocking(move || {
            arrived.send(()).unwrap();
            manager.adopt_configuration(
                &adopting_id,
                &adopting_candidate.identity,
                adopting_candidate.expected_binding,
            )
        });
        arrival.await.unwrap();
        drop(release);
        assert_eq!(adoption.await.unwrap(), Err(AdoptionError::Busy));
        fixture.gates[0].wait_entered().await;
        assert!(
            runtime
                .configuration_view()
                .unwrap()
                .admitted_attempt
                .is_some()
        );
        // The admitted attempt makes the runtime publish Busy; the
        // configuration application is untouched by it.
        eligibility.until(AdoptionEligibility::Busy).await;
        let mut settlement = SettlementReceiptWait::capture(&runtime);
        fixture.gates[0].release();
        settlement.wait().await;
        // Settlement publishes Eligible without any configuration read.
        eligibility.until(AdoptionEligibility::Eligible).await;
        fixture
            .manager
            .adopt_configuration(&id, &candidate.identity, candidate.expected_binding)
            .unwrap();
        assert_eq!(
            runtime
                .runtime_resources()
                .configuration()
                .unwrap()
                .config
                .agent
                .instructions,
            "adopt when idle"
        );
        fixture.close().await;
    })
    .await;
}

/// A background executor that signals its start, parks until released, and
/// then completes successfully; cancellation still wins while parked.
struct GatedBackgroundExecutor {
    started: tokio::sync::watch::Sender<bool>,
    release: tokio::sync::watch::Sender<bool>,
}

impl GatedBackgroundExecutor {
    fn new() -> (
        Self,
        tokio::sync::watch::Receiver<bool>,
        tokio::sync::watch::Sender<bool>,
    ) {
        let (started, started_rx) = tokio::sync::watch::channel(false);
        let (release, _release_rx) = tokio::sync::watch::channel(false);
        (
            Self {
                started,
                release: release.clone(),
            },
            started_rx,
            release,
        )
    }
}

impl crate::tools::executor::ToolExecutor for GatedBackgroundExecutor {
    fn start<'a>(
        &'a self,
        _invocation: crate::tools::types::ToolInvocation,
        context: crate::tools::executor::ToolExecutionContext<'a>,
    ) -> crate::tools::executor::ToolExecutionHandle<'a> {
        let started = self.started.clone();
        let mut release = self.release.subscribe();
        crate::tools::executor::ToolExecutionHandle::settled_by_operation(
            Box::pin(async move {
                started.send_replace(true);
                release
                    .wait_for(|released| *released)
                    .await
                    .expect("release channel stays open");
                crate::tools::types::ToolExecutionResult {
                    deliveries: Vec::new(),
                    status: crate::tools::types::ToolExecutionStatus::Success,
                    content: Vec::new(),
                    duration_ms: 0,
                    exit_code: None,
                    artifacts: Vec::new(),
                    truncation: None,
                    workflow: None,
                    managed_output: None,
                }
            }),
            context.cancellation.clone(),
        )
    }

    fn progress_capability(&self) -> crate::tools::deadline::ToolProgressCapability {
        crate::tools::deadline::ToolProgressCapability::None
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn live_model_selection_accepts_background_ownership_without_adopting_pending_context() {
    bounded(async {
        let fixture = Fixture::with_session_count(None, 1).await;
        let id = fixture.sessions[0].id.clone();
        fixture.manager.load(&id, None).await.unwrap();
        let runtime = fixture.manager.configuration_runtime(&id).unwrap();
        let instructions = runtime
            .runtime_resources()
            .configuration()
            .unwrap()
            .config
            .agent
            .instructions
            .clone();
        let (executor, mut started, release) = GatedBackgroundExecutor::new();
        let executor: Arc<dyn crate::tools::executor::ToolExecutor> = Arc::new(executor);
        let invocation = crate::tools::types::ToolInvocation {
            id: crate::tools::types::ToolInvocationId::Agent {
                call_id: crate::runtime::identity::ToolCallId::new("live-selection-background"),
            },
            tool_id: crate::runtime::identity::ToolId::new("tool-bash"),
            tool_name: "bash".to_owned(),
            mode: crate::tools::types::ToolInvocationMode::Background,
            arguments: serde_json::json!({}),
        };
        let background = runtime.tool_runtime().background();
        let prepared = background
            .prepare_dispatch(
                &invocation,
                &executor,
                crate::tools::environment::ToolEnvironment::new(),
            )
            .unwrap();
        let crate::tools::background::BackgroundDispatchOutcome::Accepted { execution_id, .. } =
            background
                .commit_dispatch(prepared, &crate::runtime::CancellationSignal::new())
                .unwrap()
        else {
            panic!("background dispatch accepted")
        };
        started.wait_for(|entered| *entered).await.unwrap();
        write(
            &fixture,
            0,
            ConfigMutation::Instructions {
                authored: Some("pending context must remain pending".into()),
            },
        )
        .await;
        let candidate = settled(&fixture, 0).await.candidate.unwrap();
        assert_eq!(
            fixture.manager.adopt_configuration(
                &id,
                &candidate.identity,
                candidate.expected_binding,
            ),
            Err(AdoptionError::Busy)
        );

        let selected = crate::model::session::SessionModelConfig::of(
            crate::model::catalog::ModelRef::parse("local/b").unwrap(),
        );
        fixture
            .manager
            .set_model(&id, selected.clone())
            .await
            .unwrap();
        assert_eq!(runtime.model_view().configured, selected);
        assert_eq!(
            fixture
                .manager
                .sessions
                .read_settings(&id)
                .await
                .unwrap()
                .1
                .model,
            Some(selected)
        );
        assert!(
            background.configuration_busy(),
            "selection retains physical background ownership"
        );
        assert_eq!(
            runtime
                .runtime_resources()
                .configuration()
                .unwrap()
                .config
                .agent
                .instructions,
            instructions
        );
        let candidate = settled(&fixture, 0).await.candidate.unwrap();
        assert_eq!(
            fixture.manager.adopt_configuration(
                &id,
                &candidate.identity,
                candidate.expected_binding,
            ),
            Err(AdoptionError::Busy)
        );

        release.send_replace(true);
        background.wait_until_terminal(&execution_id).await.unwrap();
        fixture.gates[0].release();
        fixture.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t11_background_execution_busy_settles_into_eligible_adoption() {
    bounded(async {
        let fixture = Fixture::with_session_count(None, 1).await;
        let id = fixture.sessions[0].id.clone();
        fixture.manager.load(&id, None).await.unwrap();
        let runtime = fixture.manager.configuration_runtime(&id).unwrap();
        write(
            &fixture,
            0,
            ConfigMutation::Instructions {
                authored: Some("adopt after background settles".into()),
            },
        )
        .await;
        let candidate = settled(&fixture, 0).await.candidate.unwrap();
        let probe = fixture
            .manager
            .probe(&fixture.sessions[0].active_conversation_id);
        let preparations = probe.configuration_preparations.load(Ordering::SeqCst);
        let mut eligibility = EligibilityStream::open(&fixture, &id);
        eligibility.until(AdoptionEligibility::Eligible).await;
        let (executor, mut started, release) = GatedBackgroundExecutor::new();
        let executor: Arc<dyn crate::tools::executor::ToolExecutor> = Arc::new(executor);
        let invocation = crate::tools::types::ToolInvocation {
            id: crate::tools::types::ToolInvocationId::Agent {
                call_id: crate::runtime::identity::ToolCallId::new("call-background-gate"),
            },
            tool_id: crate::runtime::identity::ToolId::new("tool-bash"),
            tool_name: "bash".to_owned(),
            mode: crate::tools::types::ToolInvocationMode::Background,
            arguments: serde_json::json!({}),
        };
        let prepared = runtime
            .tool_runtime()
            .background()
            .prepare_dispatch(
                &invocation,
                &executor,
                crate::tools::environment::ToolEnvironment::new(),
            )
            .unwrap();
        let crate::tools::background::BackgroundDispatchOutcome::Accepted { execution_id, .. } =
            runtime
                .tool_runtime()
                .background()
                .commit_dispatch(
                    prepared,
                    &crate::runtime::cancellation::CancellationSignal::new(),
                )
                .unwrap()
        else {
            panic!("accepted");
        };
        started.wait_for(|entered| *entered).await.unwrap();
        // Attribution: no foreground attempt and no subagent owns work; the
        // background registry alone owns the Busy lifecycle.
        assert!(
            runtime
                .subagent_registry()
                .is_none_or(|s| !s.configuration_busy())
        );
        assert!(runtime.tool_runtime().background().configuration_busy());
        assert_eq!(
            runtime.idle_epoch(),
            Err(crate::runtime::conversation_runtime::IdleBusyReason::Background)
        );
        eligibility.until(AdoptionEligibility::Busy).await;
        assert_eq!(
            fixture.manager.adopt_configuration(
                &id,
                &candidate.identity,
                candidate.expected_binding
            ),
            Err(AdoptionError::Busy)
        );
        release.send_replace(true);
        let terminal = runtime
            .tool_runtime()
            .background()
            .wait_until_terminal(&execution_id)
            .await
            .unwrap();
        assert!(matches!(
            terminal.state,
            crate::tools::background::BackgroundLifecycle::Succeeded
        ));
        // The execution's own terminal notification is consumed by one
        // continuation attempt; it, not the eligibility transition, is the
        // only model traffic settlement causes.
        fixture.gates[0].wait_entered().await;
        let requests = fixture.provider.request_bodies();
        assert_eq!(requests.len(), 1, "{requests:?}");
        assert!(
            requests[0].contains("Background execution"),
            "the single request is the background terminal continuation: {}",
            requests[0]
        );
        // Background settlement can publish an earlier Eligible before the
        // continuation owns its Attempt. Capture the held continuation's read
        // cut so its later release cannot consume that obsolete transition.
        eligibility = EligibilityStream::open(&fixture, &id);
        assert_eq!(eligibility.current, AdoptionEligibility::Busy);
        fixture.gates[0].release();
        // The runtime publishes busy -> idle itself when the last owner
        // releases its admission; nothing polls.
        eligibility.until(AdoptionEligibility::Eligible).await;
        let view = fixture.manager.configuration_application(&id).unwrap();
        let pending = view.candidate.as_ref().unwrap();
        assert_eq!(pending.identity, candidate.identity);
        assert_eq!(pending.expected_binding, candidate.expected_binding);
        assert_eq!(
            probe.configuration_preparations.load(Ordering::SeqCst),
            preparations,
            "settlement prepared nothing"
        );
        assert_eq!(
            fixture.provider.request_bodies().len(),
            1,
            "no model traffic beyond the terminal continuation"
        );
        assert_eq!(
            fixture.manager.adopt_configuration(
                &id,
                &candidate.identity,
                candidate.expected_binding + 1
            ),
            Err(AdoptionError::Conflict)
        );
        let applied = fixture
            .manager
            .adopt_configuration(&id, &candidate.identity, candidate.expected_binding)
            .unwrap();
        assert_eq!(
            applied.units[&ApplyUnit::Instructions],
            UnitApplication::Applied
        );
        assert!(applied.candidate.is_none());
        assert_eq!(
            fixture.provider.request_bodies().len(),
            1,
            "adoption invoked no model"
        );
        fixture.close().await;
    })
    .await;
}
