//! Headless configuration contracts. Watch edges acknowledge native commits;
//! no delay or polling interval is used to establish ordering.
#![allow(clippy::large_futures)] // bounded fixture futures; no recursive or unbounded stack growth
use super::*;
use crate::local_runtime::configuration::application::{
    AdoptionError, ApplyUnit, ConfigurationApplication, UnitApplication,
};
use crate::local_runtime::configuration::settings::{ConfigMutation, SourceMutation, SourceScope};

async fn settled(fixture: &Fixture, index: usize) -> ConfigurationApplication {
    let mut changed = fixture.manager.configuration_changes();
    loop {
        if let Some(view) = fixture
            .manager
            .configuration_application(&fixture.sessions[index].id)
            && view
                .units
                .values()
                .all(|unit| !matches!(unit, UnitApplication::Preparing))
        {
            return view;
        }
        changed.changed().await.unwrap();
    }
}

async fn write(fixture: &Fixture, index: usize, mutation: ConfigMutation) {
    let id = &fixture.sessions[index].id;
    let (source, _, _) = fixture.manager.source_settings(id, None).await.unwrap();
    fixture
        .manager
        .source_settings(
            id,
            Some((
                source.user.revision,
                SourceMutation::Config {
                    scope: SourceScope::User,
                    mutation,
                },
            )),
        )
        .await
        .unwrap();
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
        .reconcile_configuration(&fixture.sessions[1].id)
        .await
        .unwrap();
    let healthy = settled(&fixture, 1).await;
    assert!(healthy.candidate.is_none());
    assert_eq!(runtimes[1].runtime_resources().revision(), revision);
    fixture.manager.drain_all_runtimes().await;
}

#[tokio::test]
async fn t11_concrete_candidate_conflict_and_t13_same_source_retry() {
    let fixture = Fixture::new().await;
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
    let old = settled(&fixture, 0).await.candidate.unwrap();
    fixture.manager.reconcile_configuration(id).await.unwrap();
    let newer = settled(&fixture, 0).await.candidate.unwrap();
    assert_eq!(old.identity.input_revision, newer.identity.input_revision);
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
    let fixture = Fixture::new().await;
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
        let fixture = Fixture::new().await;
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
        let settlement = runtime.settlement_signal();
        fixture.gates[0].release();
        settlement.notified().await;
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
        settlement.notified().await;
        fixture.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t06_offside_preparation_allows_admission_and_t07_new_failure_supersedes_old_candidate() {
    bounded(async {
        let fixture = Fixture::new().await;
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
        fixture.manager.reconcile_configuration(id).await.unwrap();
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
        assert!(latest.candidate.is_none());
        assert!(Arc::ptr_eq(&before, &runtime.runtime_resources()));
        fixture.gates[0].release();
        runtime.settlement_signal().notified().await;
        fixture.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t11_admission_gate_orders_busy_adoption_without_cancelling_attempt() {
    bounded(async {
        let fixture = Fixture::new().await;
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
        let settlement = runtime.settlement_signal();
        fixture.gates[0].release();
        settlement.notified().await;
        runtime.wait_for_configuration_admissions().await;
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

#[tokio::test]
async fn t03_mixed_application_and_true_process_binding_restart() {
    let fixture = Fixture::new().await;
    let id = &fixture.sessions[0].id;
    fixture.manager.load(id, None).await.unwrap();
    let (source, _, _) = fixture.manager.source_settings(id, None).await.unwrap();
    let mut document: toml::Value =
        toml::from_str(&std::fs::read_to_string(&source.user.path).unwrap()).unwrap();
    document["agent"]
        .as_table_mut()
        .unwrap()
        .insert("instructions".into(), "pending context".into());
    document
        .as_table_mut()
        .unwrap()
        .insert("approval_mode".into(), "full_access".into());
    let desired = crate::local_runtime::app_server_policy::AppServerPolicy {
        shutdown_deadline_ms: 45_000,
        max_connections: 9,
        ..Default::default()
    };
    document
        .as_table_mut()
        .unwrap()
        .insert("app_server".into(), toml::Value::try_from(desired).unwrap());
    std::fs::write(&source.user.path, toml::to_string(&document).unwrap()).unwrap();
    fixture.manager.reconcile_configuration(id).await.unwrap();
    let application = settled(&fixture, 0).await;
    assert_eq!(
        application.units[&ApplyUnit::ExecutionPolicy],
        UnitApplication::Applied
    );
    assert!(matches!(
        application.units[&ApplyUnit::Instructions],
        UnitApplication::Ready { .. }
    ));
    assert_eq!(
        application.units[&ApplyUnit::ProcessBindings],
        UnitApplication::ProcessRestart
    );
    assert_eq!(fixture.host.policy().max_connections, 9);
    assert_eq!(fixture.host.policy().shutdown_deadline_ms, 30_000);
    let runtime = fixture.manager.configuration_runtime(id).unwrap();
    assert_eq!(
        runtime.configuration_view().unwrap().approval_mode,
        crate::runtime::ApprovalMode::FullAccess
    );
    assert_ne!(
        runtime
            .runtime_resources()
            .configuration()
            .unwrap()
            .config
            .agent
            .instructions,
        "pending context"
    );
    fixture.close().await;
}

#[tokio::test]
async fn t05_complete_policy_registry_publishes_while_instructions_remain_pending() {
    let fixture = Fixture::new().await;
    let id = &fixture.sessions[0].id;
    fixture.manager.load(id, None).await.unwrap();
    let runtime = fixture.manager.configuration_runtime(id).unwrap();
    let old = runtime.runtime_resources();
    write(
        &fixture,
        0,
        ConfigMutation::Instructions {
            authored: Some("pending instructions".into()),
        },
    )
    .await;
    assert!(settled(&fixture, 0).await.candidate.is_some());
    write(
        &fixture,
        0,
        ConfigMutation::NativePolicy {
            id: crate::local_runtime::configuration::settings::NativeTool::Read,
            authored: Some(
                serde_json::from_value(serde_json::json!({"approval":"always"})).unwrap(),
            ),
        },
    )
    .await;
    let application = settled(&fixture, 0).await;
    assert_eq!(
        application.units[&ApplyUnit::Capabilities],
        UnitApplication::Applied,
        "{application:?}"
    );
    assert!(application.candidate.is_some());
    let current = runtime.runtime_resources();
    assert_eq!(
        old.configuration().unwrap().config.agent.instructions,
        current.configuration().unwrap().config.agent.instructions
    );
    let read = |resources: &crate::runtime::RuntimeResourceSnapshot| {
        resources
            .capability()
            .tool_registry()
            .definitions()
            .into_iter()
            .find(|tool| tool.name == "read")
            .unwrap()
            .approval_policy
    };
    assert_ne!(read(&old), read(&current));
    assert_eq!(
        read(&current),
        crate::tools::types::ToolApprovalPolicy::Always
    );
    assert_eq!(
        runtime
            .configuration_view()
            .unwrap()
            .available_tools
            .iter()
            .find(|tool| tool.name == "read")
            .unwrap()
            .approval_policy,
        read(&current)
    );
    fixture.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t08_model_baseline_change_rejects_prepared_context() {
    bounded(async {
        let fixture = Fixture::new().await;
        let id = &fixture.sessions[0].id;
        fixture.manager.load(id, None).await.unwrap();
        let runtime = fixture.manager.configuration_runtime(id).unwrap();
        let probe = fixture
            .manager
            .probe(&fixture.sessions[0].active_conversation_id);
        probe.before_configuration_publish.arm();
        write(
            &fixture,
            0,
            ConfigMutation::Instructions {
                authored: Some("prepared for old model".into()),
            },
        )
        .await;
        probe.before_configuration_publish.entered().await;
        let candidate_id = fixture
            .manager
            .configuration_application(id)
            .unwrap()
            .desired;
        // A deliberate model commit uses the same runtime gate and advances
        // the Session baseline while preparation holds no admission lock.
        runtime
            .model_set(crate::model::session::SessionModelConfig::of(
                crate::model::catalog::ModelRef::parse("local/b").unwrap(),
            ))
            .unwrap();
        probe.before_configuration_publish.release();
        let application = settled(&fixture, 0).await;
        assert!(
            application.candidate.is_none(),
            "stale baseline cannot become ready"
        );
        assert!(matches!(
            application.units[&ApplyUnit::Instructions],
            UnitApplication::Failed { .. }
        ));
        assert_eq!(runtime.model_view().configured.model.to_string(), "local/b");
        assert_ne!(
            runtime
                .runtime_resources()
                .configuration()
                .unwrap()
                .config
                .agent
                .instructions,
            "prepared for old model"
        );
        assert_eq!(application.desired, candidate_id);
        fixture.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t12_lost_source_write_response_does_not_cancel_native_application() {
    bounded(async {
        let fixture = Fixture::new().await;
        let id = fixture.sessions[0].id.clone();
        fixture.manager.load(&id, None).await.unwrap();
        let probe = fixture
            .manager
            .probe(&fixture.sessions[0].active_conversation_id);
        probe.after_configuration_persistence.arm();
        probe.before_configuration_prepare.arm();
        let (source, _, _) = fixture.manager.source_settings(&id, None).await.unwrap();
        let manager = fixture.manager.clone();
        let target = id.clone();
        let rpc = tokio::spawn(async move {
            manager
                .source_settings(
                    &target,
                    Some((
                        source.user.revision,
                        SourceMutation::Config {
                            scope: SourceScope::User,
                            mutation: ConfigMutation::Instructions {
                                authored: Some("survives disconnect".into()),
                            },
                        },
                    )),
                )
                .await
        });
        // The source commit and ownership transfer have happened; the caller
        // cannot yet receive a response. Drop exactly that caller's future.
        probe.after_configuration_persistence.entered().await;
        rpc.abort();
        assert!(rpc.await.unwrap_err().is_cancelled());
        probe.before_configuration_prepare.entered().await;
        let preparing = fixture.manager.configuration_application(&id).unwrap();
        assert_eq!(
            fixture
                .manager
                .adopt_configuration(&id, &preparing.desired, 1),
            Err(AdoptionError::NotReady)
        );
        probe.before_configuration_prepare.release();
        let ready = settled(&fixture, 0).await;
        assert!(ready.candidate.is_some());
        assert_eq!(ready.desired, preparing.desired);
        assert!(ready.version > preparing.version);
        // Reconnect rereads authority. It does not repeat the source mutation.
        let (reread, _, _) = fixture.manager.source_settings(&id, None).await.unwrap();
        assert_eq!(reread.application, Some(ready));
        probe.after_configuration_persistence.release();
        fixture.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t02_later_tool_batch_and_model_step_keep_admitted_registry_policy() {
    Box::pin(bounded(async {
        let fixture = Fixture::with_tool(Some("read")).await;
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
            ConfigMutation::NativePolicy {
                id: crate::local_runtime::configuration::settings::NativeTool::Read,
                authored: Some(
                    serde_json::from_value(serde_json::json!({"approval":"always"})).unwrap(),
                ),
            },
        )
        .await;
        let applied = settled(&fixture, 0).await;
        assert_eq!(
            applied.units[&ApplyUnit::Capabilities],
            UnitApplication::Applied
        );
        assert!(runtime.runtime_resources().revision() > captured.generation);
        let settlement = runtime.settlement_signal();
        // Only now can the provider release the Tool proposal. The Tool batch
        // and subsequent model Step are created after newer policy published.
        fixture.gates[0].release();
        settlement.notified().await;
        let requests = fixture.provider.request_bodies();
        assert_eq!(
            requests.len(),
            2,
            "old policy executes the Tool without new approval and reaches the next Step"
        );
        let second: serde_json::Value = serde_json::from_str(&requests[1]).unwrap();
        assert!(
            second["messages"]
                .as_array()
                .unwrap()
                .iter()
                .any(|message| message["role"] == "tool")
        );
        fixture.close().await;
    }))
    .await;
}

#[tokio::test]
async fn t09_unselected_model_and_default_edits_are_noop_for_existing_session_t15_new_default() {
    let fixture = Fixture::new().await;
    let id = &fixture.sessions[0].id;
    fixture.manager.load(id, None).await.unwrap();
    let runtime = fixture.manager.configuration_runtime(id).unwrap();
    let old = runtime.runtime_resources();
    let (source, _, _) = fixture.manager.source_settings(id, None).await.unwrap();
    let path = source.user.path;
    let mut document: toml::Value =
        toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    document["models"]["local/b"]["id"] = "b-redefined".into();
    document["agent"]["model"]["model"] = "local/b".into();
    std::fs::write(path, toml::to_string(&document).unwrap()).unwrap();
    fixture.manager.reconcile_configuration(id).await.unwrap();
    let applied = settled(&fixture, 0).await;
    assert!(applied.candidate.is_none(), "{applied:?}");
    assert!(
        applied
            .units
            .values()
            .all(|unit| *unit == UnitApplication::Applied)
    );
    assert_eq!(old.revision(), runtime.runtime_resources().revision());
    assert_eq!(runtime.model_view().configured.model.to_string(), "local/a");
    let created = fixture
        .manager
        .create_session(SessionPersistentState {
            cwd: fixture.workspaces[0].clone(),
            model: None,
        })
        .await
        .unwrap();
    fixture
        .manager
        .load(&created.session.id, None)
        .await
        .unwrap();
    let new = fixture
        .manager
        .configuration_runtime(&created.session.id)
        .unwrap();
    assert_eq!(new.model_view().configured.model.to_string(), "local/b");
    assert_eq!(runtime.model_view().configured.model.to_string(), "local/a");
    fixture.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t13_failed_preparation_retries_same_input_and_t14_latest_pending_is_bounded() {
    Box::pin(bounded(async {
        let fixture = Fixture::new().await;
        let id = &fixture.sessions[0].id;
        fixture.manager.load(id, None).await.unwrap();
        let probe = fixture
            .manager
            .probe(&fixture.sessions[0].active_conversation_id);
        probe.fail_configuration_once.store(true, Ordering::SeqCst);
        write(
            &fixture,
            0,
            ConfigMutation::Instructions {
                authored: Some("retry this input".into()),
            },
        )
        .await;
        let failed = settled(&fixture, 0).await;
        assert!(matches!(
            failed.units[&ApplyUnit::Instructions],
            UnitApplication::Failed { .. }
        ));
        fixture.manager.reconcile_configuration(id).await.unwrap();
        let retried = settled(&fixture, 0).await;
        assert_eq!(
            retried.desired.input_revision,
            failed.desired.input_revision
        );
        assert!(retried.desired.attempt > failed.desired.attempt);
        assert!(retried.candidate.is_some());
        let before = probe.configuration_preparations.load(Ordering::SeqCst);
        probe.before_configuration_prepare.arm();
        write(
            &fixture,
            0,
            ConfigMutation::Instructions {
                authored: Some("superseded".into()),
            },
        )
        .await;
        probe.before_configuration_prepare.entered().await;
        for n in 0..12 {
            write(
                &fixture,
                0,
                ConfigMutation::Instructions {
                    authored: Some(format!("latest {n}")),
                },
            )
            .await;
        }
        assert_eq!(
            probe.configuration_preparations.load(Ordering::SeqCst),
            before + 1,
            "one active preparation; saves only replace pending input"
        );
        let latest = fixture
            .manager
            .configuration_application(id)
            .unwrap()
            .desired;
        probe.before_configuration_prepare.release();
        let ready = settled(&fixture, 0).await;
        assert_eq!(ready.candidate.as_ref().unwrap().identity, latest);
        assert_eq!(
            probe.configuration_preparations.load(Ordering::SeqCst),
            before + 2,
            "only the latest pending input is prepared after old work settles"
        );
        let candidate = ready.candidate.unwrap();
        fixture
            .manager
            .adopt_configuration(id, &candidate.identity, candidate.expected_binding)
            .unwrap();
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
            "latest 11"
        );
        fixture.close().await;
    }))
    .await;
}

#[tokio::test]
async fn t08_model_capture_ignores_unrelated_resource_directories_t09_same_selection_noop() {
    let fixture = Fixture::new().await;
    let id = &fixture.sessions[0].id;
    fixture.manager.load(id, None).await.unwrap();
    let directory = fixture.workspaces[0].join(".agents");
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(directory.join("unrelated.txt"), "resource manifest entry").unwrap();
    let selected = crate::model::session::SessionModelConfig::of(
        crate::model::catalog::ModelRef::parse("local/b").unwrap(),
    );
    let view = fixture
        .manager
        .set_model(id, selected.clone())
        .await
        .unwrap();
    assert_eq!(view.configured, selected);
    settled(&fixture, 0).await;
    let runtime = fixture.manager.configuration_runtime(id).unwrap();
    let before = runtime.runtime_resources();
    let application = fixture.manager.configuration_application(id).unwrap();
    fixture.manager.set_model(id, selected).await.unwrap();
    assert!(Arc::ptr_eq(&before, &runtime.runtime_resources()));
    assert_eq!(
        fixture.manager.configuration_application(id).unwrap(),
        application
    );
    fixture.close().await;
}
