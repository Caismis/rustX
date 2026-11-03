//! Automatic configuration commit by live Sessions: native commits a saved
//! configuration at its own safe boundary with no client action, admitted
//! Attempts keep their capture, real owners defer the commit until they
//! settle, newer desire supersedes staged work, and commit orders against
//! independent admission.
#![allow(clippy::large_futures)] // bounded fixture futures; no recursive or unbounded stack growth
use super::*;

fn instructions(runtime: &crate::runtime::ConversationRuntime) -> String {
    runtime
        .runtime_resources()
        .configuration()
        .unwrap()
        .config
        .agent
        .instructions
        .clone()
}

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
        applied(&fixture, 0).await;
        let runtime = fixture.manager.configuration_runtime(id).unwrap();
        runtime.submit_inbound(input("request-A")).unwrap();
        fixture.gates[0].wait_entered().await;
        let admitted =
            crate::local_runtime::configuration::settings::EffectiveConfiguration::adopted(
                &runtime,
            )
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
        // Prepared and staged: the running Attempt owns the current generation.
        let staged = settled(&fixture, 0).await;
        assert_eq!(
            staged.units[&ApplyUnit::Capabilities],
            UnitApplication::Ready,
            "{staged:?}"
        );
        assert!(runtime.has_staged_configuration());
        assert_eq!(
            crate::local_runtime::configuration::settings::EffectiveConfiguration::adopted(
                &runtime
            )
            .unwrap()
            .admitted_attempt
            .unwrap(),
            admitted
        );
        let mut settlement = SettlementReceiptWait::capture(&runtime);
        fixture.gates[0].release();
        settlement.wait().await;
        // Settlement alone commits the staged candidate: no save, click or
        // message drives it.
        let committed = applied(&fixture, 0).await;
        assert_eq!(
            committed.units[&ApplyUnit::Capabilities],
            UnitApplication::Applied
        );
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
                "every step of the admitted Attempt retains Todo"
            );
        }
        let change = configuration_changes(&runtime).pop().unwrap();
        assert_eq!(change.tools.removed.names, ["todo"]);
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
async fn t04_every_resident_session_commits_and_t09_policy_and_healthy_rescan_are_noops() {
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
    let runtimes: Vec<_> = fixture
        .sessions
        .iter()
        .map(|s| fixture.manager.configuration_runtime(&s.id).unwrap())
        .collect();
    for (index, runtime) in runtimes.iter().enumerate() {
        let view = applied(&fixture, index).await;
        assert_eq!(
            view.units[&ApplyUnit::Instructions],
            UnitApplication::Applied
        );
        assert_eq!(instructions(runtime), "P2");
        let changes = configuration_changes(runtime);
        assert_eq!(changes.len(), 1, "{changes:?}");
        assert_eq!(
            changes[0].domains,
            [crate::runtime::configuration_commit::ConfigurationDomain::Instructions]
        );
        assert!(changes[0].tools.is_empty());
    }
    write(
        &fixture,
        0,
        ConfigMutation::Approval {
            authored: Some(crate::runtime::ApprovalMode::FullAccess),
        },
    )
    .await;
    applied(&fixture, 0).await;
    applied(&fixture, 1).await;
    for runtime in &runtimes {
        assert_eq!(instructions(runtime), "P2");
        assert_eq!(
            crate::local_runtime::configuration::settings::EffectiveConfiguration::adopted(runtime)
                .unwrap()
                .approval_mode,
            crate::runtime::ApprovalMode::FullAccess
        );
    }
    let revision = runtimes[1].runtime_resources().revision();
    let changes = configuration_changes(&runtimes[1]).len();
    fixture
        .manager
        .reconcile_configuration(
            &crate::local_runtime::configuration::settings::SourceTarget::Workspace {
                directory: fixture.workspaces[1].clone(),
            },
        )
        .await
        .unwrap();
    applied(&fixture, 1).await;
    assert_eq!(runtimes[1].runtime_resources().revision(), revision);
    assert_eq!(
        configuration_changes(&runtimes[1]).len(),
        changes,
        "an unchanged source records no change"
    );
    assert!(
        fixture.provider.request_bodies().is_empty(),
        "commit invokes no model"
    );
    fixture.manager.drain_all_runtimes().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t11_newer_desire_supersedes_a_staged_candidate_and_rescan_does_not_reprepare() {
    bounded(async {
        let fixture = Fixture::with_session_count(None, 1).await;
        let id = &fixture.sessions[0].id;
        fixture.manager.load(id, None).await.unwrap();
        let runtime = fixture.manager.configuration_runtime(id).unwrap();
        runtime.submit_inbound(input("request-A")).unwrap();
        fixture.gates[0].wait_entered().await;
        write(
            &fixture,
            0,
            ConfigMutation::Instructions {
                authored: Some("candidate one".into()),
            },
        )
        .await;
        let staged = settled(&fixture, 0).await;
        assert_eq!(
            staged.units[&ApplyUnit::Instructions],
            UnitApplication::Ready
        );
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
        assert_eq!(settled(&fixture, 0).await, staged);
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
        let newer = settled(&fixture, 0).await;
        assert_ne!(newer.desired, staged.desired);
        assert_eq!(
            newer.units[&ApplyUnit::Instructions],
            UnitApplication::Ready
        );
        assert_ne!(instructions(&runtime), "candidate one");
        let mut settlement = SettlementReceiptWait::capture(&runtime);
        fixture.gates[0].release();
        settlement.wait().await;
        applied(&fixture, 0).await;
        assert_eq!(instructions(&runtime), "candidate two");
        assert_eq!(
            configuration_changes(&runtime).len(),
            1,
            "the superseded candidate never committed"
        );
        fixture.close().await;
    })
    .await;
}

#[tokio::test]
async fn t15_unload_load_retains_the_committed_configuration_without_replay() {
    let fixture = Fixture::with_session_count(None, 1).await;
    let id = &fixture.sessions[0].id;
    let loaded = fixture.manager.load(id, None).await.unwrap();
    write(
        &fixture,
        0,
        ConfigMutation::Instructions {
            authored: Some("committed before unload".into()),
        },
    )
    .await;
    applied(&fixture, 0).await;
    let runtime = fixture.manager.configuration_runtime(id).unwrap();
    assert_eq!(instructions(&runtime), "committed before unload");
    assert_eq!(configuration_changes(&runtime).len(), 1);
    fixture.manager.unload(&loaded.conversation).await.unwrap();
    fixture.manager.load(id, None).await.unwrap();
    let reloaded = fixture.manager.configuration_runtime(id).unwrap();
    assert_eq!(instructions(&reloaded), "committed before unload");
    assert_eq!(
        configuration_changes(&reloaded).len(),
        1,
        "reconstruction never replays a historical change"
    );
    assert!(
        fixture.provider.request_bodies().is_empty(),
        "commit and reload invoke no model"
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
        let captured =
            crate::local_runtime::configuration::settings::EffectiveConfiguration::adopted(
                &runtime,
            )
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
        assert_eq!(
            crate::local_runtime::configuration::settings::EffectiveConfiguration::adopted(
                &runtime
            )
            .unwrap()
            .approval_mode,
            crate::runtime::ApprovalMode::FullAccess
        );
        assert_eq!(
            crate::local_runtime::configuration::settings::EffectiveConfiguration::adopted(
                &runtime
            )
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
        let later = crate::local_runtime::configuration::settings::EffectiveConfiguration::adopted(
            &runtime,
        )
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
            "[agent]\nagents=['missing']\n",
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
        assert!(matches!(
            latest.units[&ApplyUnit::Capabilities],
            UnitApplication::Failed { .. }
        ));
        // The User instructions remain desired and independently valid; they
        // are staged under the newest identity while the Attempt owns the
        // generation.
        assert_eq!(
            latest.units[&ApplyUnit::Instructions],
            UnitApplication::Ready
        );
        assert!(Arc::ptr_eq(&before, &runtime.runtime_resources()));
        let mut settlement = SettlementReceiptWait::capture(&runtime);
        fixture.gates[0].release();
        settlement.wait().await;
        let committed = applied(&fixture, 0).await;
        assert_eq!(
            committed.units[&ApplyUnit::Instructions],
            UnitApplication::Applied
        );
        assert!(matches!(
            committed.units[&ApplyUnit::Capabilities],
            UnitApplication::Failed { .. }
        ));
        assert_eq!(instructions(&runtime), "superseded candidate");
        fixture.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t11_ready_candidate_commits_before_racing_independent_admission() {
    bounded(async {
        let fixture = Fixture::with_session_count(None, 1).await;
        let id = fixture.sessions[0].id.clone();
        fixture.manager.load(&id, None).await.unwrap();
        let runtime = fixture.manager.configuration_runtime(&id).unwrap();
        // Park the admission pass before it takes the coordinator lock.
        let gate = Arc::new(crate::runtime::conversation_runtime::Gate::default());
        let release = gate.arm_scoped();
        runtime.install_configuration_admission_gate(gate.clone());
        runtime.submit_inbound(input("request-A")).unwrap();
        tokio::task::spawn_blocking(move || gate.wait_entered())
            .await
            .unwrap();
        write(
            &fixture,
            0,
            ConfigMutation::Instructions {
                authored: Some("committed before admission".into()),
            },
        )
        .await;
        applied(&fixture, 0).await;
        drop(release);
        fixture.gates[0].wait_entered().await;
        // The commit linearized first, so the independent Attempt froze it.
        let admitted =
            crate::local_runtime::configuration::settings::EffectiveConfiguration::adopted(
                &runtime,
            )
            .unwrap()
            .admitted_attempt
            .unwrap();
        assert_eq!(admitted.generation, runtime.runtime_resources().revision());
        let request = fixture.provider.request_bodies().pop().unwrap();
        assert!(request.contains("committed before admission"), "{request}");
        let mut settlement = SettlementReceiptWait::capture(&runtime);
        fixture.gates[0].release();
        settlement.wait().await;
        fixture.close().await;
    })
    .await;
}

/// A background executor that signals its start, parks until released, and
/// then completes successfully; cancellation still wins while parked.
pub(super) struct GatedBackgroundExecutor {
    started: tokio::sync::watch::Sender<bool>,
    release: tokio::sync::watch::Sender<bool>,
}

impl GatedBackgroundExecutor {
    pub(super) fn new() -> (
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

/// Accept one gated background execution directly on the runtime.
fn start_background(
    runtime: &crate::runtime::ConversationRuntime,
    call: &str,
) -> (
    crate::runtime::identity::ToolExecutionId,
    tokio::sync::watch::Receiver<bool>,
    tokio::sync::watch::Sender<bool>,
) {
    let (executor, started, release) = GatedBackgroundExecutor::new();
    let executor: Arc<dyn crate::tools::executor::ToolExecutor> = Arc::new(executor);
    let invocation = crate::tools::types::ToolInvocation {
        id: crate::tools::types::ToolInvocationId::Agent {
            call_id: crate::runtime::identity::ToolCallId::new(call),
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
    (execution_id, started, release)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn live_model_selection_supersedes_a_candidate_blocked_by_background_ownership() {
    bounded(async {
        let fixture = Fixture::with_session_count(None, 1).await;
        let id = fixture.sessions[0].id.clone();
        fixture.manager.load(&id, None).await.unwrap();
        let runtime = fixture.manager.configuration_runtime(&id).unwrap();
        let original = instructions(&runtime);
        let (execution_id, mut started, release) =
            start_background(&runtime, "live-selection-background");
        started.wait_for(|entered| *entered).await.unwrap();
        write(
            &fixture,
            0,
            ConfigMutation::Instructions {
                authored: Some("commits after background settles".into()),
            },
        )
        .await;
        let staged = settled(&fixture, 0).await;
        assert_eq!(
            staged.units[&ApplyUnit::Instructions],
            UnitApplication::Ready
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
        let background = runtime.tool_runtime().background();
        assert!(
            background.configuration_busy(),
            "selection retains physical background ownership"
        );
        assert_eq!(instructions(&runtime), original);
        // Selection superseded the staged candidate; the source is prepared
        // again over the explicit selection and stays staged.
        let reprepared = settled(&fixture, 0).await;
        assert_ne!(reprepared.desired, staged.desired);
        assert_eq!(
            reprepared.units[&ApplyUnit::Instructions],
            UnitApplication::Ready
        );

        release.send_replace(true);
        background.wait_until_terminal(&execution_id).await.unwrap();
        fixture.gates[0].wait_entered().await;
        fixture.gates[0].release();
        applied(&fixture, 0).await;
        assert_eq!(instructions(&runtime), "commits after background settles");
        assert_eq!(
            runtime.model_view().configured,
            selected,
            "configuration application never overwrites explicit selection"
        );
        fixture.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t11_background_ownership_defers_commit_until_it_settles() {
    bounded(async {
        let fixture = Fixture::with_session_count(None, 1).await;
        let id = fixture.sessions[0].id.clone();
        fixture.manager.load(&id, None).await.unwrap();
        let runtime = fixture.manager.configuration_runtime(&id).unwrap();
        let original = instructions(&runtime);
        let (execution_id, mut started, release) =
            start_background(&runtime, "call-background-gate");
        started.wait_for(|entered| *entered).await.unwrap();
        // Attribution: no foreground attempt and no subagent owns work; the
        // background registry alone owns the generation.
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
        write(
            &fixture,
            0,
            ConfigMutation::Instructions {
                authored: Some("commit after background settles".into()),
            },
        )
        .await;
        let staged = settled(&fixture, 0).await;
        assert_eq!(
            staged.units[&ApplyUnit::Instructions],
            UnitApplication::Ready
        );
        assert_eq!(instructions(&runtime), original);
        let probe = fixture
            .manager
            .probe(&fixture.sessions[0].active_conversation_id);
        let preparations = probe.configuration_preparations.load(Ordering::SeqCst);
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
        // continuation attempt; it, not the commit, is the only model traffic
        // settlement causes.
        fixture.gates[0].wait_entered().await;
        fixture.gates[0].release();
        let committed = applied(&fixture, 0).await;
        assert_eq!(
            committed.units[&ApplyUnit::Instructions],
            UnitApplication::Applied
        );
        assert_eq!(instructions(&runtime), "commit after background settles");
        assert_eq!(
            probe.configuration_preparations.load(Ordering::SeqCst),
            preparations,
            "settlement prepared nothing"
        );
        let requests = fixture.provider.request_bodies();
        assert_eq!(requests.len(), 1, "{requests:?}");
        assert!(
            requests[0].contains("Background execution"),
            "the single request is the background terminal continuation: {}",
            requests[0]
        );
        fixture.close().await;
    })
    .await;
}

/// Issue #518: an external edit of a supported Workspace source reaches the
/// same native path as a Save through the bounded watcher — a notification is
/// only invalidation — and commits automatically, recording one change fact.
/// An edit of an ordinary project file in the same Workspace changes nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn external_workspace_edit_commits_through_the_bounded_source_watch() {
    bounded(async {
        let fixture = Fixture::with_session_count(None, 1).await;
        let id = fixture.sessions[0].id.clone();
        fixture.manager.load(&id, None).await.unwrap();
        let runtime = fixture.manager.configuration_runtime(&id).unwrap();
        let before = configuration_changes(&runtime).len();
        let stop = tokio_util::sync::CancellationToken::new();
        let edit = async {
            let mut changed = fixture.manager.configuration_changes();
            std::fs::write(fixture.workspaces[0].join("notes.txt"), "not a source").unwrap();
            let path = fixture.workspaces[0].join("rustx.toml");
            let mut config: toml::Value =
                toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            config.as_table_mut().unwrap().insert(
                "agent".into(),
                toml::Value::try_from(
                    serde_json::json!({"instructions": "from an external editor"}),
                )
                .unwrap(),
            );
            std::fs::write(&path, toml::to_string(&config).unwrap()).unwrap();
            while instructions(&runtime) != "from an external editor" {
                changed.changed().await.unwrap();
            }
            stop.cancel();
        };
        let (watched, ()) = tokio::join!(fixture.manager.run_source_watch(stop.clone()), edit);
        watched.unwrap();
        let changes = configuration_changes(&runtime);
        assert_eq!(changes.len(), before + 1, "{changes:?}");
        assert!(
            changes[before]
                .domains
                .contains(&crate::runtime::configuration_commit::ConfigurationDomain::Instructions)
        );
        fixture.close().await;
    })
    .await;
}
