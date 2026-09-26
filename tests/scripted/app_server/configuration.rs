//! Headless configuration contracts. Watch edges acknowledge native commits;
//! no delay or polling interval is used to establish ordering.
#![allow(clippy::large_futures)] // bounded fixture futures; no recursive or unbounded stack growth
use super::*;
use crate::local_runtime::configuration::application::{
    AdoptionError, ApplyUnit, ConfigurationApplication, UnitApplication,
};
use crate::local_runtime::configuration::settings::{ConfigMutation, SourceMutation};

async fn source_settled(
    f: &Fixture,
    target: &crate::local_runtime::configuration::settings::SourceTarget,
) -> ConfigurationApplication {
    let mut changes = f.manager.configuration_changes();
    loop {
        if let Some(application) = f
            .manager
            .applications
            .lock()
            .view(&target.application_scope())
            && application
                .units
                .values()
                .all(|unit| !matches!(unit, UnitApplication::Preparing))
        {
            return application;
        }
        changes.changed().await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn c07_zero_session_commit_transfers_ownership_before_lost_response() {
    use crate::local_runtime::configuration::settings::SourceTarget;
    bounded(async {
        let f = Fixture::with_session_count(None, 0).await;
        let source = f
            .manager
            .source_settings(&SourceTarget::User, None)
            .await
            .unwrap();
        let (entered, waiting) = tokio::sync::oneshot::channel();
        let (release, resume) = std::sync::mpsc::channel();
        f.manager
            .configuration
            .test_hooks
            .insert("after_coordination_transfer", move || {
                entered.send(()).unwrap();
                resume.recv().unwrap();
            });
        let manager = f.manager.clone();
        let rpc = tokio::spawn(async move {
            manager
                .source_settings(
                    &SourceTarget::User,
                    Some((
                        source.user.revision,
                        SourceMutation::Config {
                            mutation: ConfigMutation::AppServer {
                                authored: Some(
                                    crate::local_runtime::app_server_policy::AppServerPolicy {
                                        max_connections: 19,
                                        ..Default::default()
                                    },
                                ),
                            },
                        },
                    )),
                )
                .await
        });
        waiting.await.unwrap(); // Persistence and transfer happened; RPC has not returned.
        rpc.abort();
        assert!(rpc.await.unwrap_err().is_cancelled());
        let applied = source_settled(&f, &SourceTarget::User).await;
        assert_eq!(
            applied.units[&ApplyUnit::ProcessBindings],
            UnitApplication::Applied
        );
        assert_eq!(f.manager.process_policy().max_connections, 19);
        assert!(f.manager.registry.0.lock().unwrap().entries.is_empty());
        release.send(()).unwrap();
        assert_eq!(
            f.manager
                .source_settings(&SourceTarget::User, None)
                .await
                .unwrap()
                .user
                .authored
                .unwrap()
                .app_server
                .unwrap()
                .max_connections,
            19
        );
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn c01_c02_c03_zero_session_source_authority_is_inert_and_isolated() {
    use crate::local_runtime::configuration::settings::SourceTarget;
    bounded(async {
        let f = Fixture::with_session_count(None, 0).await;
        for target in [
            SourceTarget::User,
            SourceTarget::Workspace {
                directory: f.workspaces[0].clone(),
            },
        ] {
            let source = f.manager.source_settings(&target, None).await.unwrap();
            assert_eq!(source.workspace.is_some(), target.workspace().is_some());
            assert_eq!(
                source.workspace_resource_root.is_some(),
                target.workspace().is_some()
            );
            let revision = match target {
                SourceTarget::User => source.user.revision,
                SourceTarget::Workspace { .. } => source.workspace.unwrap().revision,
            };
            f.manager
                .source_settings(
                    &target,
                    Some((
                        revision,
                        SourceMutation::Config {
                            mutation: ConfigMutation::NativeTools {
                                authored: Some(vec![]),
                            },
                        },
                    )),
                )
                .await
                .unwrap();
            source_settled(&f, &target).await;
        }
        assert!(
            f.manager
                .sessions
                .list_sessions(None, 0, 32)
                .await
                .unwrap()
                .sessions
                .is_empty()
        );
        assert!(f.manager.registry.0.lock().unwrap().entries.is_empty());
        assert!(
            f.manager
                .sessions
                .configuration_bindings
                .lock()
                .unwrap()
                .is_empty()
        );
        assert!(f.provider.request_bodies().is_empty());
        assert!(
            f.manager
                .applications
                .lock()
                .views()
                .iter()
                .all(|a| a.scope.starts_with("source:") && a.units.len() == 1)
        );
        let user = f
            .manager
            .source_settings(&SourceTarget::User, None)
            .await
            .unwrap();
        assert!(
            !serde_json::to_string(&user)
                .unwrap()
                .contains(f.workspaces[0].to_str().unwrap())
        );
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn c06_zero_session_process_hot_restart_reopen_and_revert() {
    use crate::local_runtime::configuration::settings::SourceTarget;
    bounded(async {
        let f = Fixture::with_session_count(None, 0).await;
        let target = SourceTarget::User;
        let actual = f.manager.process_policy();
        for restart in [false, true, false] {
            let source = f.manager.source_settings(&target, None).await.unwrap();
            let mut desired = actual.clone();
            desired.max_connections = 17;
            if restart {
                desired.shutdown_deadline_ms += 1;
            }
            f.manager
                .source_settings(
                    &target,
                    Some((
                        source.user.revision,
                        SourceMutation::Config {
                            mutation: ConfigMutation::AppServer {
                                authored: Some(desired),
                            },
                        },
                    )),
                )
                .await
                .unwrap();
            let application = source_settled(&f, &target).await;
            assert_eq!(
                application.units[&ApplyUnit::ProcessBindings],
                if restart {
                    UnitApplication::ProcessRestart
                } else {
                    UnitApplication::Applied
                }
            );
            let reopened = f.manager.source_settings(&target, None).await.unwrap();
            assert_eq!(reopened.application.unwrap(), application);
            let process = reopened.process_bindings.unwrap();
            assert_eq!(process.max_connections, 17);
            assert_eq!(process.shutdown_deadline_ms, actual.shutdown_deadline_ms);
            assert_eq!(
                reopened
                    .user
                    .authored
                    .unwrap()
                    .app_server
                    .unwrap()
                    .shutdown_deadline_ms,
                actual.shutdown_deadline_ms + u64::from(restart)
            );
        }
        assert!(f.manager.registry.0.lock().unwrap().entries.is_empty());
        assert!(
            f.manager
                .sessions
                .list_sessions(None, 0, 32)
                .await
                .unwrap()
                .sessions
                .is_empty()
        );
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn c05_broken_source_retains_revision_inventory_and_explicit_validated_repair() {
    use crate::local_runtime::configuration::settings::SourceTarget;
    bounded(async {
        let f = Fixture::with_session_count(None, 0).await;
        let target = SourceTarget::User;
        let initial = f.manager.source_settings(&target, None).await.unwrap();
        let bytes = std::fs::read_to_string(&initial.user.path).unwrap();
        std::fs::write(&initial.user.path, "broken = [").unwrap();
        let broken = f.manager.source_settings(&target, None).await.unwrap();
        assert_ne!(broken.user.revision, initial.user.revision);
        assert!(broken.user.authored.is_none());
        assert!(broken.user.diagnostic.is_some());
        assert!(broken.prospective_resources.is_some());
        assert!(
            f.manager
                .source_settings(
                    &target,
                    Some((
                        broken.user.revision.clone(),
                        SourceMutation::RepairConfig {
                            document: "still = [".into()
                        }
                    ))
                )
                .await
                .is_err()
        );
        let repaired = f
            .manager
            .source_settings(
                &target,
                Some((
                    broken.user.revision,
                    SourceMutation::RepairConfig { document: bytes },
                )),
            )
            .await
            .unwrap();
        assert!(repaired.user.authored.is_some());
        assert!(repaired.user.diagnostic.is_none());
        // Syntactically valid but unresolved sources retain the entire editor and
        // independently valid inventory; a default model is not read admission.
        let mut document: toml::Value =
            toml::from_str(&std::fs::read_to_string(&repaired.user.path).unwrap()).unwrap();
        document["agent"].as_table_mut().unwrap().remove("model");
        std::fs::write(&repaired.user.path, toml::to_string(&document).unwrap()).unwrap();
        let missing = f.manager.source_settings(&target, None).await.unwrap();
        assert!(missing.user.authored.is_some());
        assert!(missing.prospective_diagnostic.is_some());
        assert!(missing.prospective_resources.is_some());
        document["agent"].as_table_mut().unwrap().insert(
            "model".into(),
            toml::Value::try_from(serde_json::json!({"model":"local/missing"})).unwrap(),
        );
        std::fs::write(&repaired.user.path, toml::to_string(&document).unwrap()).unwrap();
        let unresolved = f.manager.source_settings(&target, None).await.unwrap();
        assert!(unresolved.user.authored.is_some());
        assert!(unresolved.prospective_diagnostic.is_some());
        assert!(unresolved.prospective_resources.is_some());
        assert!(f.manager.registry.0.lock().unwrap().entries.is_empty());
        assert!(f.provider.request_bodies().is_empty());
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn c04_workspace_override_empty_and_removal_keep_distinct_source_intent() {
    use crate::local_runtime::configuration::settings::SourceTarget;
    bounded(async {
        let f = Fixture::with_session_count(None, 0).await;
        let target = SourceTarget::Workspace {
            directory: f.workspaces[0].clone(),
        };
        let original = f.manager.source_settings(&target, None).await.unwrap();
        for authored in [Some(vec!["read".into()]), Some(vec![]), None] {
            let before = f.manager.source_settings(&target, None).await.unwrap();
            let after = f
                .manager
                .source_settings(
                    &target,
                    Some((
                        before.workspace.unwrap().revision,
                        SourceMutation::Config {
                            mutation: ConfigMutation::NativeTools {
                                authored: authored.clone(),
                            },
                        },
                    )),
                )
                .await
                .unwrap();
            assert_eq!(after.user, original.user);
            assert_eq!(
                after
                    .workspace
                    .unwrap()
                    .authored
                    .unwrap()
                    .agent
                    .and_then(|a| a.tools)
                    .and_then(|t| t.builtin),
                authored
            );
            let resolved = after
                .resolved
                .unwrap()
                .agent
                .unwrap()
                .tools
                .unwrap()
                .builtin;
            assert_eq!(
                resolved,
                authored.or_else(|| original
                    .user
                    .authored
                    .as_ref()
                    .unwrap()
                    .agent
                    .as_ref()
                    .unwrap()
                    .tools
                    .as_ref()
                    .unwrap()
                    .builtin
                    .clone())
            );
        }
        assert!(f.manager.registry.0.lock().unwrap().entries.is_empty());
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn c11_c12_model_selection_and_source_authoring_have_disjoint_durable_owners() {
    use crate::local_runtime::configuration::settings::SourceTarget;
    bounded(async {
        let f = Fixture::new().await;
        f.manager.load(&f.sessions[0].id, None).await.unwrap();
        f.manager.load(&f.sessions[1].id, None).await.unwrap();
        let target = SourceTarget::Workspace {
            directory: f.workspaces[0].clone(),
        };
        let sources = f.manager.source_settings(&target, None).await.unwrap();
        let other = f
            .manager
            .sessions
            .read_settings(&f.sessions[1].id)
            .await
            .unwrap();
        let selection = crate::model::session::SessionModelConfig::of(
            crate::model::catalog::ModelRef::parse("local/b").unwrap(),
        );
        f.manager
            .set_model(&f.sessions[0].id, selection.clone())
            .await
            .unwrap();
        let selected = f
            .manager
            .sessions
            .read_settings(&f.sessions[0].id)
            .await
            .unwrap();
        assert_eq!(selected.1.model, Some(selection));
        assert_eq!(
            f.manager
                .sessions
                .read_settings(&f.sessions[1].id)
                .await
                .unwrap(),
            other
        );
        let after = f.manager.source_settings(&target, None).await.unwrap();
        assert_eq!(after.user, sources.user);
        assert_eq!(after.workspace, sources.workspace);
        f.manager
            .source_settings(
                &target,
                Some((
                    after.workspace.unwrap().revision,
                    SourceMutation::Config {
                        mutation: ConfigMutation::Instructions {
                            authored: Some("new source guidance".into()),
                        },
                    },
                )),
            )
            .await
            .unwrap();
        settled(&f, 0).await;
        assert_eq!(
            f.manager
                .sessions
                .read_settings(&f.sessions[0].id)
                .await
                .unwrap(),
            selected
        );
        assert_eq!(
            f.manager
                .sessions
                .read_settings(&f.sessions[1].id)
                .await
                .unwrap(),
            other
        );
        assert!(f.provider.request_bodies().is_empty());
        f.close().await;
    })
    .await;
}

pub(super) async fn settled(fixture: &Fixture, index: usize) -> ConfigurationApplication {
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

async fn write(fixture: &Fixture, _index: usize, mutation: ConfigMutation) {
    let source = fixture
        .manager
        .source_settings(
            &crate::local_runtime::configuration::settings::SourceTarget::User,
            None,
        )
        .await
        .unwrap();
    fixture
        .manager
        .source_settings(
            &crate::local_runtime::configuration::settings::SourceTarget::User,
            Some((source.user.revision, SourceMutation::Config { mutation })),
        )
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue383_disabled_extension_keeps_admitted_configuration_until_settlement() {
    Box::pin(bounded(async {
        let fixture = Fixture::with_tool(Some("read")).await;
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
        let settlement = runtime.settlement_signal();
        fixture.gates[0].release();
        settlement.notified().await;
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
        let settlement = runtime.settlement_signal();
        fixture.gates[1].release();
        settlement.notified().await;
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
        assert_eq!(
            fixture
                .manager
                .configuration_application(&id)
                .unwrap()
                .eligibility,
            crate::local_runtime::configuration::application::AdoptionEligibility::Busy
        );
        let settlement = runtime.settlement_signal();
        fixture.gates[0].release();
        settlement.notified().await;
        runtime.wait_for_configuration_admissions().await;
        assert_eq!(
            fixture
                .manager
                .configuration_application(&id)
                .unwrap()
                .eligibility,
            crate::local_runtime::configuration::application::AdoptionEligibility::Eligible
        );
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
async fn t11_background_execution_busy_settles_into_eligible_adoption() {
    bounded(async {
        let fixture = Fixture::new().await;
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
        assert_eq!(
            fixture
                .manager
                .configuration_application(&id)
                .unwrap()
                .eligibility,
            crate::local_runtime::configuration::application::AdoptionEligibility::Eligible
        );
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
        assert!(runtime.subagents().is_none_or(|s| !s.configuration_busy()));
        assert!(runtime.tool_runtime().background().configuration_busy());
        assert_eq!(
            runtime.idle_epoch(),
            Err(crate::runtime::conversation_runtime::IdleBusyReason::Background)
        );
        assert_eq!(
            fixture
                .manager
                .configuration_application(&id)
                .unwrap()
                .eligibility,
            crate::local_runtime::configuration::application::AdoptionEligibility::Busy
        );
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
        fixture.gates[0].release();
        // No notification fires for busy -> idle: eligibility is computed at
        // read time, so poll the manager read inside the liveness guard.
        loop {
            let view = fixture.manager.configuration_application(&id).unwrap();
            if view.eligibility
                == crate::local_runtime::configuration::application::AdoptionEligibility::Eligible
            {
                let pending = view.candidate.as_ref().unwrap();
                assert_eq!(pending.identity, candidate.identity);
                assert_eq!(pending.expected_binding, candidate.expected_binding);
                break;
            }
            tokio::task::yield_now().await;
        }
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

#[tokio::test]
async fn t03_mixed_application_and_true_process_binding_restart() {
    let fixture = Fixture::new().await;
    let id = &fixture.sessions[0].id;
    fixture.manager.load(id, None).await.unwrap();
    let source = fixture
        .manager
        .source_settings(
            &crate::local_runtime::configuration::settings::SourceTarget::User,
            None,
        )
        .await
        .unwrap();
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
    fixture
        .manager
        .reconcile_configuration(
            &crate::local_runtime::configuration::settings::SourceTarget::Workspace {
                directory: fixture.workspaces[0].clone(),
            },
        )
        .await
        .unwrap();
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
        let (entered, waiting) = tokio::sync::oneshot::channel();
        let (release, resume) = std::sync::mpsc::channel();
        fixture
            .manager
            .configuration
            .test_hooks
            .insert("after_coordination_transfer", move || {
                entered.send(()).unwrap();
                resume.recv().unwrap();
            });
        probe.before_configuration_prepare.arm();
        let source = fixture
            .manager
            .source_settings(
                &crate::local_runtime::configuration::settings::SourceTarget::User,
                None,
            )
            .await
            .unwrap();
        let manager = fixture.manager.clone();
        let target = crate::local_runtime::configuration::settings::SourceTarget::User;
        let rpc = tokio::spawn(async move {
            manager
                .source_settings(
                    &target,
                    Some((
                        source.user.revision,
                        SourceMutation::Config {
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
        waiting.await.unwrap();
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
        let reread = fixture
            .manager
            .source_settings(
                &crate::local_runtime::configuration::settings::SourceTarget::User,
                None,
            )
            .await
            .unwrap();
        assert_eq!(reread.application.unwrap().scope, "source:user");
        assert_eq!(
            fixture
                .manager
                .configuration_application(&id)
                .unwrap()
                .desired,
            ready.desired
        );
        release.send(()).unwrap();
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
    let source = fixture
        .manager
        .source_settings(
            &crate::local_runtime::configuration::settings::SourceTarget::User,
            None,
        )
        .await
        .unwrap();
    let path = source.user.path;
    let mut document: toml::Value =
        toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    document["models"]["local/b"]["id"] = "b-redefined".into();
    document["agent"]["model"]["model"] = "local/b".into();
    std::fs::write(path, toml::to_string(&document).unwrap()).unwrap();
    fixture
        .manager
        .reconcile_configuration(
            &crate::local_runtime::configuration::settings::SourceTarget::Workspace {
                directory: fixture.workspaces[0].clone(),
            },
        )
        .await
        .unwrap();
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
        fixture
            .manager
            .reconcile_configuration(
                &crate::local_runtime::configuration::settings::SourceTarget::Workspace {
                    directory: fixture.workspaces[0].clone(),
                },
            )
            .await
            .unwrap();
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t03_t05_mixed_instructions_publication_rejects_rebound_project_path() {
    bounded(async {
        let fixture = Fixture::new().await;
        let id = &fixture.sessions[0].id;
        let workspace = &fixture.workspaces[0];
        let old_path = workspace.join("old.md");
        let new_path = workspace.join("new.md");
        std::fs::write(&old_path, "I1 project content").unwrap();
        std::fs::write(&new_path, "I2 project content").unwrap();
        let skill = workspace.join(".agents/skills/retained");
        std::fs::create_dir_all(&skill).unwrap();
        std::fs::write(
            skill.join("SKILL.md"),
            "---\nname: retained\ndescription: Retained guidance\n---\nC1 skill guidance\n",
        )
        .unwrap();
        std::fs::write(
            workspace.join("rustx.toml"),
            "[agent]\ninstructions='I1'\n[agent.agents_md]\nfiles=['old.md']\n",
        )
        .unwrap();
        fixture.manager.load(id, None).await.unwrap();
        let runtime = fixture.manager.configuration_runtime(id).unwrap();
        let before = runtime.runtime_resources();
        let old = before.configuration().unwrap();
        assert_eq!(old.config.agent.agents_md.files, vec![old_path.clone()]);
        assert!(!before.capability().skills().packages().is_empty());

        std::fs::write(
            workspace.join("rustx.toml"),
            "[agent]\ninstructions='I2'\n[agent.agents_md]\nfiles=['new.md']\n[agent.tools]\nbuiltin=['read']\n",
        )
        .unwrap();
        let probe = fixture.manager.probe(&fixture.sessions[0].active_conversation_id);
        probe.fail_configuration_once.store(true, Ordering::SeqCst);
        probe.before_configuration_publish.arm();
        fixture.manager.reconcile_configuration(&crate::local_runtime::configuration::settings::SourceTarget::Workspace { directory: fixture.workspaces[0].clone() }).await.unwrap();
        // This gate is after C2 construction/failure and independent I2
        // preparation, but before make_available's physical-authority fence.
        probe.before_configuration_publish.entered().await;
        let preparing = fixture.manager.configuration_application(id).unwrap();
        assert!(matches!(
            &preparing.units[&ApplyUnit::Capabilities],
            UnitApplication::Failed { diagnostic }
                if diagnostic.contains("after resource construction")
        ), "{preparing:?}");
        assert!(matches!(preparing.units[&ApplyUnit::Instructions], UnitApplication::Preparing));
        let outside = fixture.workspaces[1].join("outside.md");
        std::fs::write(&outside, "outside Workspace").unwrap();
        std::fs::remove_file(&new_path).unwrap();
        std::os::unix::fs::symlink(&outside, &new_path).unwrap();
        probe.before_configuration_publish.release();
        let rejected = settled(&fixture, 0).await;
        assert!(matches!(
            &rejected.units[&ApplyUnit::Instructions],
            UnitApplication::Failed { diagnostic }
                if diagnostic.contains("new.md") && diagnostic.contains("outside workspace boundary")
        ), "{rejected:?}");
        assert!(rejected.candidate.is_none());
        assert!(Arc::ptr_eq(&before, &runtime.runtime_resources()));

        // Keep the escaping link in place: admission can succeed only using
        // the previous available C1+I1, including old.md's physical authority.
        let second = fixture.manager.create_session(SessionPersistentState {
            cwd: workspace.clone(),
            model: None,
        }).await.unwrap().session.id;
        let retained = fixture.manager.sessions.configuration_bindings.lock().unwrap()[&second].clone();
        assert_eq!(retained.config.agent.instructions, "I1");
        assert_eq!(retained.config.agent.agents_md.files, vec![old_path]);
        assert_eq!(retained.effective.agent, old.effective.agent);
        assert_eq!(retained.effective.context, old.effective.context);
        assert_eq!(retained.root_agent_project_files, before.root_profile().unwrap().project_instructions.files);
        assert_eq!(retained.component_revisions, old.component_revisions);
        assert_eq!(retained.source_revisions, old.source_revisions);
        retained.validate_resource_authority().unwrap();
        fixture.manager.load(&second, None).await.unwrap();
        let loaded = fixture.manager.configuration_runtime(&second).unwrap().runtime_resources();
        assert_eq!(loaded.root_profile().unwrap().project_instructions.files,
            before.root_profile().unwrap().project_instructions.files);
        assert_eq!(loaded.configuration().unwrap().config.agent.instructions, "I1");
        assert_eq!(loaded.capability().tool_registry().model_definitions(),
            before.capability().tool_registry().model_definitions());
        assert_eq!(loaded.capability().skills().packages(), before.capability().skills().packages());
        let after = runtime.runtime_resources();
        assert!(Arc::ptr_eq(before.capability().tool_registry(), after.capability().tool_registry()));
        assert!(Arc::ptr_eq(before.capability().skills(), after.capability().skills()));
        assert_eq!(before.capability().revision(), after.capability().revision());
        fixture.close().await;
    }).await;
}

#[tokio::test]
async fn t03_t05_failed_capabilities_preserve_leases_while_instructions_are_adopted() {
    let fixture = Fixture::new().await;
    let id = &fixture.sessions[0].id;
    let skill = fixture.workspaces[0].join(".agents/skills/retained");
    std::fs::create_dir_all(&skill).unwrap();
    std::fs::write(
        skill.join("SKILL.md"),
        "---\nname: retained\ndescription: Retained guidance\n---\nC1 skill guidance\n",
    )
    .unwrap();
    let initial_source = fixture
        .manager
        .source_settings(
            &crate::local_runtime::configuration::settings::SourceTarget::User,
            None,
        )
        .await
        .unwrap();
    let mut initial: toml::Value =
        toml::from_str(&std::fs::read_to_string(&initial_source.user.path).unwrap()).unwrap();
    initial["agent"]
        .as_table_mut()
        .unwrap()
        .insert("instructions".into(), "User I1".into());
    std::fs::write(
        &initial_source.user.path,
        toml::to_string(&initial).unwrap(),
    )
    .unwrap();
    std::fs::write(
        fixture.workspaces[0].join("old.md"),
        "Workspace I1 project input",
    )
    .unwrap();
    std::fs::write(
        fixture.workspaces[0].join("rustx.toml"),
        "[agent.agents_md]\nfiles=['old.md']\n",
    )
    .unwrap();
    fixture.manager.load(id, None).await.unwrap();
    let runtime = fixture.manager.configuration_runtime(id).unwrap();
    let before = runtime.runtime_resources();
    let source = fixture
        .manager
        .source_settings(
            &crate::local_runtime::configuration::settings::SourceTarget::User,
            None,
        )
        .await
        .unwrap();
    let mut document: toml::Value =
        toml::from_str(&std::fs::read_to_string(&source.user.path).unwrap()).unwrap();
    let project_file = fixture.workspaces[0].join("new.md");
    std::fs::write(&project_file, "Workspace I2 project input").unwrap();
    std::fs::write(
        fixture.workspaces[0].join("rustx.toml"),
        "approval_mode='full_access'\n[subagents]\nmax_concurrent=3\n[agent]\ninstructions='independent instructions P2'\n[agent.agents_md]\nfiles=['new.md']\n",
    )
    .unwrap();
    document["agent"]["tools"]["builtin"] = toml::Value::try_from(vec!["read"]).unwrap();
    std::fs::write(&source.user.path, toml::to_string(&document).unwrap()).unwrap();
    fixture
        .manager
        .probe(&fixture.sessions[0].active_conversation_id)
        .fail_configuration_once
        .store(true, Ordering::SeqCst);
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
    assert!(
        matches!(
            application.units[&ApplyUnit::Capabilities],
            UnitApplication::Failed { .. }
        ),
        "{application:?}"
    );
    assert!(
        matches!(
            application.units[&ApplyUnit::Instructions],
            UnitApplication::Ready { .. }
        ),
        "{application:?}"
    );
    let candidate = application.candidate.unwrap();
    let adopted = fixture
        .manager
        .adopt_configuration(id, &candidate.identity, candidate.expected_binding)
        .unwrap();
    assert!(matches!(
        adopted.units[&ApplyUnit::Capabilities],
        UnitApplication::Failed { .. }
    ));
    let after = runtime.runtime_resources();
    assert_eq!(
        after.configuration().unwrap().config.agent.instructions,
        "independent instructions P2"
    );
    assert_eq!(
        before.capability().revision(),
        after.capability().revision()
    );
    assert!(Arc::ptr_eq(
        before.capability().skills(),
        after.capability().skills()
    ));
    assert_eq!(
        before.configuration().unwrap().component_revisions[&ApplyUnit::Capabilities],
        after.configuration().unwrap().component_revisions[&ApplyUnit::Capabilities]
    );
    assert_ne!(
        before.configuration().unwrap().component_revisions[&ApplyUnit::Instructions],
        after.configuration().unwrap().component_revisions[&ApplyUnit::Instructions]
    );
    assert!(Arc::ptr_eq(
        before.capability().tool_registry(),
        after.capability().tool_registry()
    ));
    assert_eq!(
        before.configuration().unwrap().config.native_tools,
        after.configuration().unwrap().config.native_tools
    );
    let create = || {
        fixture.manager.create_session(SessionPersistentState {
            cwd: fixture.workspaces[0].clone(),
            model: None,
        })
    };
    let second = create().await.unwrap().session.id;
    let retained = |id: &SessionId| {
        fixture
            .manager
            .sessions
            .configuration_bindings
            .lock()
            .unwrap()[id]
            .clone()
    };
    let c1i2 = retained(&second);
    assert_eq!(
        c1i2.config.agent.agents_md.files,
        vec![project_file.clone()]
    );
    assert_eq!(
        c1i2.root_agent_project_files,
        after.root_profile().unwrap().project_instructions.files
    );
    assert!(matches!(
        c1i2.provenance["agent.agents_md"],
        crate::local_runtime::configuration::Origin::Workspace { .. }
    ));
    c1i2.validate_resource_authority().unwrap();
    assert!(
        c1i2.root_agent_project_files
            .iter()
            .any(|file| file.path == project_file && file.content == "Workspace I2 project input")
    );

    assert_eq!(
        c1i2.config.approval_mode,
        crate::runtime::ApprovalMode::FullAccess
    );
    assert_eq!(
        c1i2.effective.approval_mode,
        Some(c1i2.config.approval_mode)
    );
    assert!(matches!(
        c1i2.provenance["approval_mode"],
        crate::local_runtime::configuration::Origin::Workspace { .. }
    ));
    assert_eq!(c1i2.config.subagents.max_concurrent, 3);
    assert_eq!(
        c1i2.effective.subagents.as_ref().unwrap().max_concurrent,
        Some(3)
    );
    assert!(matches!(
        c1i2.provenance["subagents.max_concurrent"],
        crate::local_runtime::configuration::Origin::Workspace { .. }
    ));
    for unit in [ApplyUnit::ExecutionPolicy, ApplyUnit::SharedCapacity] {
        assert_ne!(
            c1i2.component_revisions[&unit],
            c1i2.component_revisions[&ApplyUnit::Capabilities]
        );
        assert_eq!(
            c1i2.component_revisions[&unit],
            after.configuration().unwrap().component_revisions[&unit]
        );
    }

    assert!(fixture.manager.applications.is_deferred(second.as_str()));
    assert!(matches!(
        before.configuration().unwrap().provenance["agent.instructions"],
        crate::local_runtime::configuration::Origin::User { .. }
    ));
    assert!(matches!(
        c1i2.provenance["agent.instructions"],
        crate::local_runtime::configuration::Origin::Workspace { .. }
    ));
    assert!(matches!(
        after.configuration().unwrap().provenance["agent.instructions"],
        crate::local_runtime::configuration::Origin::Workspace { .. }
    ));
    assert_eq!(
        c1i2.effective
            .agent
            .as_ref()
            .unwrap()
            .instructions
            .as_deref(),
        Some("independent instructions P2")
    );
    assert_eq!(
        c1i2.component_revisions[&ApplyUnit::Instructions],
        after.configuration().unwrap().component_revisions[&ApplyUnit::Instructions]
    );
    assert_eq!(
        c1i2.source_revisions,
        before.configuration().unwrap().source_revisions
    );
    assert_eq!(
        after.configuration().unwrap().source_revisions,
        before.configuration().unwrap().source_revisions
    );
    assert_eq!(
        c1i2.config.agent.instructions,
        "independent instructions P2"
    );
    assert_eq!(
        c1i2.component_revisions[&ApplyUnit::Capabilities],
        before.configuration().unwrap().component_revisions[&ApplyUnit::Capabilities]
    );
    assert_eq!(
        c1i2.config.native_tools,
        before.configuration().unwrap().config.native_tools
    );
    assert_eq!(
        c1i2.skill_discovery.packages,
        retained(id).skill_discovery.packages
    );
    fixture.manager.load(&second, None).await.unwrap();
    let second_resources = fixture
        .manager
        .configuration_runtime(&second)
        .unwrap()
        .runtime_resources();
    assert_eq!(
        second_resources
            .configuration()
            .unwrap()
            .config
            .native_tools,
        c1i2.config.native_tools
    );
    assert_eq!(
        second_resources
            .capability()
            .tool_registry()
            .model_definitions(),
        before.capability().tool_registry().model_definitions()
    );
    assert_eq!(
        second_resources.capability().skills().packages(),
        before.capability().skills().packages()
    );
    assert_eq!(
        second_resources
            .root_profile()
            .unwrap()
            .project_instructions
            .files,
        c1i2.root_agent_project_files
    );
    assert_eq!(
        second_resources
            .configuration()
            .unwrap()
            .component_revisions,
        c1i2.component_revisions
    );
    // Retry the same authored C2; S1/S2 retain their explicit compositions.
    fixture
        .manager
        .reconcile_configuration(
            &crate::local_runtime::configuration::settings::SourceTarget::Workspace {
                directory: fixture.workspaces[0].clone(),
            },
        )
        .await
        .unwrap();
    settled(&fixture, 0).await;
    let third = create().await.unwrap().session.id;
    let c2i2 = retained(&third);
    assert_eq!(
        c2i2.config.agent.instructions,
        "independent instructions P2"
    );
    assert_ne!(
        c2i2.component_revisions[&ApplyUnit::Capabilities],
        c1i2.component_revisions[&ApplyUnit::Capabilities]
    );
    assert_ne!(c2i2.config.native_tools, c1i2.config.native_tools);
    assert!(Arc::ptr_eq(
        &second_resources,
        &fixture
            .manager
            .configuration_runtime(&second)
            .unwrap()
            .runtime_resources()
    ));

    assert_eq!(
        retained(&second).component_revisions,
        c1i2.component_revisions
    );
    assert_eq!(
        runtime
            .runtime_resources()
            .configuration()
            .unwrap()
            .config
            .agent
            .instructions,
        "independent instructions P2"
    );
    assert_eq!(
        runtime.runtime_resources().capability().revision(),
        before.capability().revision()
    );
    assert!(!before.capability().skills().packages().is_empty());
    fixture.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t09_t13_t15_new_sessions_use_available_during_preparation_failure_and_retry() {
    Box::pin(bounded(async {
        let fixture = Fixture::new().await;
        let id = &fixture.sessions[0].id;
        fixture.manager.load(id, None).await.unwrap();
        let before = fixture
            .manager
            .configuration_runtime(id)
            .unwrap()
            .runtime_resources();
        let probe = fixture
            .manager
            .probe(&fixture.sessions[0].active_conversation_id);
        probe.before_configuration_prepare.arm();
        let source = fixture
            .manager
            .source_settings(
                &crate::local_runtime::configuration::settings::SourceTarget::User,
                None,
            )
            .await
            .unwrap();
        let mut document: toml::Value =
            toml::from_str(&std::fs::read_to_string(&source.user.path).unwrap()).unwrap();
        document["agent"]["model"]["model"] = "local/b".into();
        std::fs::write(&source.user.path, toml::to_string(&document).unwrap()).unwrap();
        write(
            &fixture,
            0,
            ConfigMutation::Instructions {
                authored: Some("available after success".into()),
            },
        )
        .await;
        probe.before_configuration_prepare.entered().await;
        let create = || {
            fixture.manager.create_session(SessionPersistentState {
                cwd: fixture.workspaces[0].clone(),
                model: None,
            })
        };
        let during = create().await.unwrap().session.id;
        let retained = |id: &SessionId| {
            fixture
                .manager
                .sessions
                .configuration_bindings
                .lock()
                .unwrap()[id]
                .clone()
        };
        assert_eq!(
            retained(&during).config.agent.instructions,
            before.configuration().unwrap().config.agent.instructions
        );
        assert!(fixture.manager.configuration_runtime(&during).is_none());
        probe.fail_configuration_once.store(true, Ordering::SeqCst);
        probe.before_configuration_prepare.release();
        let failed = settled(&fixture, 0).await;
        assert!(matches!(
            failed.units[&ApplyUnit::Instructions],
            UnitApplication::Failed { .. }
        ));
        let after_failure = create().await.unwrap().session.id;
        assert_eq!(
            retained(&after_failure).config.agent.instructions,
            retained(&during).config.agent.instructions
        );
        assert_eq!(
            retained(&after_failure).session_model(),
            retained(&during).session_model()
        );
        assert!(retained(&after_failure).same_capabilities(&retained(&during)));
        fixture
            .manager
            .reconcile_configuration(
                &crate::local_runtime::configuration::settings::SourceTarget::Workspace {
                    directory: fixture.workspaces[0].clone(),
                },
            )
            .await
            .unwrap();
        let success = settled(&fixture, 0).await;
        assert_eq!(
            success.desired.input_revision,
            failed.desired.input_revision
        );
        assert!(success.candidate.is_some());
        let after_success = create().await.unwrap().session.id;
        assert_eq!(
            retained(&after_success).config.agent.instructions,
            "available after success"
        );
        assert_eq!(
            retained(&after_success).session_model().model.to_string(),
            "local/b"
        );
        assert_eq!(
            retained(&during).session_model().model.to_string(),
            "local/a"
        );
        assert_eq!(
            retained(&after_failure).session_model().model.to_string(),
            "local/a"
        );
        assert_ne!(
            retained(&after_failure).config.agent.instructions,
            "available after success"
        );
        assert_ne!(
            retained(&during).config.agent.instructions,
            "available after success"
        );
        fixture.manager.load(&after_failure, None).await.unwrap();
        assert_eq!(
            fixture
                .manager
                .configuration_runtime(&after_failure)
                .unwrap()
                .runtime_resources()
                .configuration()
                .unwrap()
                .config
                .agent
                .instructions,
            before.configuration().unwrap().config.agent.instructions
        );
        fixture.close().await;
    }))
    .await;
}

#[tokio::test]
async fn t06_t15_configuration_save_keeps_cold_sessions_outside_residency_budget() {
    let fixture = Fixture::new().await;
    let mut ids = Vec::new();
    for _ in 0..5 {
        ids.push(
            fixture
                .manager
                .create_session(SessionPersistentState {
                    cwd: fixture.workspaces[0].clone(),
                    model: None,
                })
                .await
                .unwrap()
                .session
                .id,
        );
    }
    // Persist the small process limit so source reconciliation keeps it.
    let source = fixture
        .manager
        .source_settings(
            &crate::local_runtime::configuration::settings::SourceTarget::User,
            None,
        )
        .await
        .unwrap();
    let mut document: toml::Value =
        toml::from_str(&std::fs::read_to_string(&source.user.path).unwrap()).unwrap();
    document.as_table_mut().unwrap().insert(
        "app_server".into(),
        toml::toml! { max_resident_runtimes = 1 }.into(),
    );
    std::fs::write(&source.user.path, toml::to_string(&document).unwrap()).unwrap();
    let before = fixture.manager.registry.0.lock().unwrap().entries.len();
    fixture
        .manager
        .source_settings(
            &crate::local_runtime::configuration::settings::SourceTarget::User,
            Some((
                fixture
                    .manager
                    .source_settings(
                        &crate::local_runtime::configuration::settings::SourceTarget::User,
                        None,
                    )
                    .await
                    .unwrap()
                    .user
                    .revision,
                SourceMutation::Config {
                    mutation: ConfigMutation::Instructions {
                        authored: Some("cold desired context".into()),
                    },
                },
            )),
        )
        .await
        .unwrap();
    let mut changed = fixture.manager.configuration_changes();
    loop {
        if ids
            .iter()
            .all(|id| fixture.manager.applications.is_deferred(id.as_ref()))
        {
            break;
        }
        changed.changed().await.unwrap();
    }
    assert_eq!(fixture.manager.process_policy().max_resident_runtimes, 1);
    assert_eq!(
        fixture.manager.registry.0.lock().unwrap().entries.len(),
        before
    );
    for id in &ids {
        assert!(fixture.manager.configuration_runtime(id).is_none());
        let view = fixture.manager.configuration_application(id).unwrap();
        assert_eq!(
            view.units[&ApplyUnit::ExecutionPolicy],
            UnitApplication::Applied
        );
        assert_eq!(
            view.units[&ApplyUnit::Instructions],
            UnitApplication::Preparing
        );
        assert!(
            !view
                .units
                .values()
                .any(|unit| matches!(unit, UnitApplication::Failed { .. }))
        );
    }
    let subsequent = fixture
        .manager
        .create_session(SessionPersistentState {
            cwd: fixture.workspaces[0].clone(),
            model: None,
        })
        .await
        .unwrap();
    assert_eq!(
        fixture
            .manager
            .sessions
            .configuration_bindings
            .lock()
            .unwrap()[&subsequent.session.id]
            .config
            .agent
            .instructions,
        "cold desired context"
    );
    assert!(
        fixture
            .manager
            .configuration_runtime(&subsequent.session.id)
            .is_none()
    );
    fixture.manager.load(&ids[0], None).await.unwrap();
    let runtime = fixture.manager.configuration_runtime(&ids[0]).unwrap();
    assert_ne!(
        runtime
            .runtime_resources()
            .configuration()
            .unwrap()
            .config
            .agent
            .instructions,
        "cold desired context"
    );
    loop {
        if fixture
            .manager
            .configuration_application(&ids[0])
            .unwrap()
            .candidate
            .is_some()
        {
            break;
        }
        changed.changed().await.unwrap();
    }
    assert_eq!(
        fixture.manager.registry.0.lock().unwrap().entries.len(),
        before + 1
    );
    assert_ne!(
        runtime
            .runtime_resources()
            .configuration()
            .unwrap()
            .config
            .agent
            .instructions,
        "cold desired context"
    );
    fixture.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t09_t15_new_session_during_preparation_keeps_available_binding_after_success() {
    Box::pin(bounded(async {
        let fixture = Fixture::new().await;
        let id = &fixture.sessions[0].id;
        fixture.manager.load(id, None).await.unwrap();
        let before = fixture
            .manager
            .sessions
            .configuration_bindings
            .lock()
            .unwrap()[id]
            .clone();
        let probe = fixture
            .manager
            .probe(&fixture.sessions[0].active_conversation_id);
        probe.before_configuration_prepare.arm();
        write(
            &fixture,
            0,
            ConfigMutation::Instructions {
                authored: Some("P2 available".into()),
            },
        )
        .await;
        probe.before_configuration_prepare.entered().await;
        let create = || {
            fixture.manager.create_session(SessionPersistentState {
                cwd: fixture.workspaces[0].clone(),
                model: None,
            })
        };
        let during = create().await.unwrap().session.id;
        let retained = |id: &SessionId| {
            fixture
                .manager
                .sessions
                .configuration_bindings
                .lock()
                .unwrap()[id]
                .clone()
        };
        assert_eq!(
            retained(&during).component_revisions,
            before.component_revisions
        );
        assert_eq!(retained(&during).session_model(), before.session_model());
        assert_eq!(
            retained(&during).config.agent.instructions,
            before.config.agent.instructions
        );
        probe.before_configuration_prepare.release();
        assert!(settled(&fixture, 0).await.candidate.is_some());
        let after = create().await.unwrap().session.id;
        assert_eq!(retained(&after).config.agent.instructions, "P2 available");
        assert_eq!(
            retained(&during).component_revisions,
            before.component_revisions
        );
        let loaded = fixture.manager.load(&during, None).await.unwrap();
        assert_eq!(
            fixture
                .manager
                .configuration_runtime(&during)
                .unwrap()
                .runtime_resources()
                .configuration()
                .unwrap()
                .config
                .agent
                .instructions,
            before.config.agent.instructions
        );
        let runtime = fixture.manager.configuration_runtime(&during).unwrap();
        let history = loaded
            .inspect_runtime()
            .unwrap()
            .historical_canonical_history()
            .unwrap();
        let mut changed = fixture.manager.configuration_changes();
        let candidate = loop {
            if let Some(candidate) = fixture
                .manager
                .configuration_application(&during)
                .and_then(|view| view.candidate)
            {
                break candidate;
            }
            changed.changed().await.unwrap();
        };
        assert_eq!(
            runtime
                .runtime_resources()
                .configuration()
                .unwrap()
                .config
                .agent
                .instructions,
            before.config.agent.instructions
        );
        fixture
            .manager
            .adopt_configuration(&during, &candidate.identity, candidate.expected_binding)
            .unwrap();
        assert_eq!(
            runtime
                .runtime_resources()
                .configuration()
                .unwrap()
                .config
                .agent
                .instructions,
            "P2 available"
        );
        assert_eq!(
            loaded
                .inspect_runtime()
                .unwrap()
                .historical_canonical_history()
                .unwrap(),
            history
        );
        assert!(fixture.provider.request_bodies().is_empty());
        fixture.close().await;
    }))
    .await;
}

#[tokio::test]
async fn t09_available_default_preparation_is_independent_of_retained_session_selection() {
    let fixture = Fixture::new().await;
    let id = &fixture.sessions[0].id;
    fixture.manager.load(id, None).await.unwrap();
    let runtime = fixture.manager.configuration_runtime(id).unwrap();
    let old = runtime.runtime_resources();
    let source = fixture
        .manager
        .source_settings(
            &crate::local_runtime::configuration::settings::SourceTarget::User,
            None,
        )
        .await
        .unwrap();
    let mut document: toml::Value =
        toml::from_str(&std::fs::read_to_string(&source.user.path).unwrap()).unwrap();
    document["agent"]["model"]["model"] = "local/b".into();
    document["models"].as_table_mut().unwrap().remove("local/a");
    // Require real capability construction as well as new provider/default
    // preparation; the old Session cannot bind this catalog to its selection.
    document.as_table_mut().unwrap().insert(
        "native_tools".into(),
        toml::toml! { read = { approval = "always" } }.into(),
    );
    std::fs::write(&source.user.path, toml::to_string(&document).unwrap()).unwrap();
    fixture
        .manager
        .reconcile_configuration(
            &crate::local_runtime::configuration::settings::SourceTarget::Workspace {
                directory: fixture.workspaces[0].clone(),
            },
        )
        .await
        .unwrap();
    let pending = settled(&fixture, 0).await;
    assert!(
        matches!(
            pending.units[&ApplyUnit::Provider],
            UnitApplication::Failed { .. }
        ),
        "{pending:?}"
    );
    assert_eq!(runtime.model_view().configured.model.to_string(), "local/a");
    assert!(Arc::ptr_eq(
        old.capability().tool_registry(),
        runtime.runtime_resources().capability().tool_registry()
    ));
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
    let resources = new.runtime_resources();
    assert_eq!(
        resources
            .capability()
            .tool_registry()
            .definitions()
            .into_iter()
            .find(|tool| tool.name == "read")
            .unwrap()
            .approval_policy,
        crate::tools::types::ToolApprovalPolicy::Always
    );
    assert_eq!(runtime.model_view().configured.model.to_string(), "local/a");
    fixture.close().await;
}

async fn workflow_only_agent_change(model_change: bool) {
    Box::pin(bounded(async {
        let fixture = Fixture::with_tool(Some("review")).await;
        let id = &fixture.sessions[0].id;
        let workspace = &fixture.workspaces[0];
        let agent = workspace.join(".agents/agents/reviewer.toml");
        let workflow = workspace.join(".agents/workflows/review.yaml");
        std::fs::create_dir_all(agent.parent().unwrap()).unwrap();
        std::fs::create_dir_all(workflow.parent().unwrap()).unwrap();
        let profile = |instructions: &str, model: &str| format!("description='Review'\ninstructions='{instructions}'\n[model]\nmodel='local/{model}'\n");
        std::fs::write(&agent, profile("reviewer A", "a")).unwrap();
        let schema = serde_json::json!({"type":"object","properties":{},"required":[],"additionalProperties":false});
        std::fs::write(&workflow, serde_json::to_string(&serde_json::json!({
            "description":"Review", "block": {"input":schema,"output":schema,"entry":"agent",
            "nodes":{"agent":{"type":"agent","profile":"reviewer","task":"Review","output":schema},
                     "done":{"type":"return","output":{"type":"literal","value":{}}}},
            "edges":[{"from":"agent","to":"done"}]}
        })).unwrap()).unwrap();
        std::fs::write(workspace.join("rustx.toml"), "[agent]\nworkflows=['review']\n").unwrap();
        let loaded = fixture.manager.load(id, None).await.unwrap();
        let live = loaded.inspect_runtime().unwrap();
        let before = live.runtime_resources();
        let retained_before = fixture.manager.sessions.configuration_bindings.lock().unwrap()[id].clone();
        assert!(before.configuration().unwrap().config.agent.agents.is_empty());
        let probe = fixture.manager.probe(&fixture.sessions[0].active_conversation_id);
        let preparations = probe.configuration_preparations.load(Ordering::SeqCst);
        std::fs::write(agent.parent().unwrap().join("unused.toml"), profile("unrelated", "b")).unwrap();
        fixture.manager.reconcile_configuration(&crate::local_runtime::configuration::settings::SourceTarget::Workspace { directory: fixture.workspaces[0].clone() }).await.unwrap();
        settled(&fixture, 0).await;
        assert_eq!(probe.configuration_preparations.load(Ordering::SeqCst), preparations);
        assert!(Arc::ptr_eq(&before, &live.runtime_resources()));
        live.submit_inbound(input("request-A old workflow")).unwrap();
        fixture.gates[0].wait_entered().await;
        let probe = fixture.manager.probe(&fixture.sessions[0].active_conversation_id);
        let preparations = probe.configuration_preparations.load(Ordering::SeqCst);
        let source = fixture.manager.source_settings(&crate::local_runtime::configuration::settings::SourceTarget::Workspace { directory: workspace.clone() }, None).await.unwrap();
        let authored = if model_change { profile("reviewer A", "b") } else { profile("reviewer B", "a") };
        fixture.manager.source_settings(&crate::local_runtime::configuration::settings::SourceTarget::Workspace { directory: workspace.clone() }, Some((
            source.agents.iter().find(|entry| entry.source.path == agent).unwrap().source.revision.clone(),
            SourceMutation::Agent {
                name: crate::runtime::subagent::SubagentName::parse("reviewer").unwrap(),
                authored: Some(crate::local_runtime::agent_resources::parse(&authored).unwrap()),
            },
        ))).await.unwrap();
        let application = settled(&fixture, 0).await;
        assert!(probe.configuration_preparations.load(Ordering::SeqCst) > preparations);
        assert_ne!(live.runtime_resources().capability().revision(), before.capability().revision());
        let done = live.settlement_signal().notified();
        fixture.gates[0].release();
        done.await;
        if let Some(candidate) = application.candidate {
            fixture.manager.adopt_configuration(id, &candidate.identity, candidate.expected_binding).unwrap();
        }
        let requests = fixture.provider.request_bodies();
        assert!(requests.iter().any(|body| body.contains("reviewer A")), "{requests:?}");
        assert!(!requests.iter().any(|body| body.contains("reviewer B")));
        let old_child = requests.iter().map(|body| serde_json::from_str::<serde_json::Value>(body).unwrap())
            .find(|body| body["tools"].as_array().unwrap().iter().any(|tool| tool["function"]["name"] == "workflow_output")).unwrap();
        assert_eq!(old_child["model"], "a");
        let old_count = requests.len();
        let done = live.settlement_signal().notified();
        live.submit_inbound(input("request-A new workflow")).unwrap();
        done.await;
        let requests = fixture.provider.request_bodies();
        let child = requests[old_count..].iter().map(|body| serde_json::from_str::<serde_json::Value>(body).unwrap())
            .find(|body| body["tools"].as_array().unwrap().iter().any(|tool| tool["function"]["name"] == "workflow_output")).expect("real Workflow Agent request");
        assert_eq!(child["model"], if model_change { "b" } else { "a" });
        assert!(child.to_string().contains(if model_change { "reviewer A" } else { "reviewer B" }));
        let retained = fixture.manager.sessions.configuration_bindings.lock().unwrap()[id].clone();
        assert_eq!(retained.same_provider(&retained_before), !model_change);
        // The same dependency closure protects the physical profile selected
        // during immutable capture, even though Root does not expose it.
        let moved = workspace.parent().unwrap().join("outside-agents");
        std::fs::rename(agent.parent().unwrap(), &moved).unwrap();
        std::os::unix::fs::symlink(&moved, agent.parent().unwrap()).unwrap();
        assert!(retained.validate_resource_authority().is_err());
        fixture.close().await;
    })).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t05_t09_workflow_only_agent_content_rebuilds_frozen_execution() {
    workflow_only_agent_change(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t05_t09_workflow_only_agent_model_rebuilds_frozen_execution() {
    workflow_only_agent_change(true).await;
}

#[tokio::test]
async fn t03_t06_t09_t15_t16_healthy_registration_preserves_complete_policy_authority() {
    let fixture = Fixture::new().await;
    let id = &fixture.sessions[0].id;
    fixture.manager.load(id, None).await.unwrap();
    let runtime = fixture.manager.configuration_runtime(id).unwrap();
    let probe = fixture
        .manager
        .probe(&fixture.sessions[0].active_conversation_id);
    std::fs::write(fixture.workspaces[0].join("rustx.toml"),
        "approval_mode='full_access'\n[model_timeout_policy]\nresponse_start_timeout_ms=12345\nstream_idle_timeout_ms=23456\n[tool_deadline_policy]\nhard_deadline_ms=34567\n[subagents]\nmax_concurrent=3\n").unwrap();
    // Save publishes policy independently; Instructions then explicitly adopt.
    write(
        &fixture,
        0,
        ConfigMutation::Instructions {
            authored: Some("healthy N+1".into()),
        },
    )
    .await;
    let application = settled(&fixture, 0).await;
    let independent = runtime.configuration_view().unwrap();
    assert_eq!(
        independent.approval_mode,
        crate::runtime::ApprovalMode::FullAccess
    );
    assert_eq!(
        independent.document.approval_mode,
        Some(independent.approval_mode)
    );
    assert!(matches!(
        independent.provenance["approval_mode"],
        crate::local_runtime::configuration::Origin::Workspace { .. }
    ));
    assert_eq!(
        independent
            .document
            .model_timeout_policy
            .as_ref()
            .unwrap()
            .response_start_timeout_ms,
        Some(12345)
    );
    assert_eq!(
        independent
            .document
            .tool_deadline_policy
            .as_ref()
            .unwrap()
            .hard_deadline_ms,
        Some(34567)
    );
    assert_eq!(
        independent
            .document
            .subagents
            .as_ref()
            .unwrap()
            .max_concurrent,
        Some(3)
    );
    let candidate = application.candidate.unwrap();
    fixture
        .manager
        .adopt_configuration(id, &candidate.identity, candidate.expected_binding)
        .unwrap();
    let healthy = fixture.manager.configuration_application(id).unwrap();
    assert!(
        healthy
            .units
            .values()
            .all(|unit| *unit == UnitApplication::Applied)
    );
    let resources = runtime.runtime_resources();
    let preparations = probe.configuration_preparations.load(Ordering::SeqCst);
    let created = fixture
        .manager
        .create_session(SessionPersistentState {
            cwd: fixture.workspaces[0].clone(),
            model: None,
        })
        .await
        .unwrap()
        .session;
    let second = &created.id;
    let adopted = fixture
        .manager
        .sessions
        .configuration_bindings
        .lock()
        .unwrap()[second]
        .clone();
    assert_eq!(adopted.config.agent.instructions, "healthy N+1");
    assert!(!fixture.manager.applications.is_deferred(second.as_str()));
    assert!(fixture.manager.configuration_application(second).is_none());
    let second_probe = fixture.manager.probe(&created.active_conversation_id);
    assert_eq!(
        second_probe
            .configuration_preparations
            .load(Ordering::SeqCst),
        0
    );
    fixture.manager.load(second, None).await.unwrap();
    let second_runtime = fixture.manager.configuration_runtime(second).unwrap();
    let second_resources = second_runtime.runtime_resources();
    let view = second_runtime.configuration_view().unwrap();
    assert_eq!(view.approval_mode, crate::runtime::ApprovalMode::FullAccess);
    assert_eq!(adopted.config.approval_mode, view.approval_mode);
    assert_eq!(view.document.approval_mode, Some(view.approval_mode));
    assert_eq!(view.model_timeout.response_start_timeout_ms, 12345);
    assert_eq!(
        view.document
            .model_timeout_policy
            .as_ref()
            .unwrap()
            .response_start_timeout_ms,
        Some(12345)
    );
    assert_eq!(view.model_timeout.stream_idle_timeout_ms, 23456);
    assert_eq!(
        view.document
            .model_timeout_policy
            .as_ref()
            .unwrap()
            .stream_idle_timeout_ms,
        Some(23456)
    );
    assert_eq!(view.tool_deadline.hard_deadline_ms, 34567);
    assert_eq!(
        view.document
            .tool_deadline_policy
            .as_ref()
            .unwrap()
            .hard_deadline_ms,
        Some(34567)
    );
    assert_eq!(view.child_capacity.max_concurrent, 3);
    assert_eq!(
        view.document.subagents.as_ref().unwrap().max_concurrent,
        Some(3)
    );
    for field in [
        "approval_mode",
        "model_timeout_policy.response_start_timeout_ms",
        "model_timeout_policy.stream_idle_timeout_ms",
        "tool_deadline_policy.hard_deadline_ms",
        "subagents.max_concurrent",
    ] {
        assert!(
            matches!(
                view.provenance[field],
                crate::local_runtime::configuration::Origin::Workspace { .. }
            ),
            "{field}"
        );
        assert_eq!(view.provenance[field], adopted.provenance[field]);
        assert_eq!(
            view.provenance[field],
            resources.configuration().unwrap().provenance[field]
        );
    }
    for unit in [
        ApplyUnit::ExecutionPolicy,
        ApplyUnit::SharedCapacity,
        ApplyUnit::Instructions,
    ] {
        assert_eq!(
            adopted.component_revisions[&unit],
            second_resources
                .configuration()
                .unwrap()
                .component_revisions[&unit]
        );
    }
    assert!(fixture.manager.configuration_application(second).is_none());
    assert_eq!(
        second_probe
            .configuration_preparations
            .load(Ordering::SeqCst),
        0
    );
    assert_eq!(
        probe.configuration_preparations.load(Ordering::SeqCst),
        preparations
    );
    assert!(Arc::ptr_eq(&resources, &runtime.runtime_resources()));
    fixture.manager.load(second, None).await.unwrap();
    assert!(Arc::ptr_eq(
        &second_resources,
        &second_runtime.runtime_resources()
    ));
    assert_eq!(
        second_probe
            .configuration_preparations
            .load(Ordering::SeqCst),
        0
    );
    fixture.close().await;
}

#[tokio::test]
async fn t06_t09_t16_process_restart_alone_does_not_defer_new_session() {
    let fixture = Fixture::new().await;
    let id = &fixture.sessions[0].id;
    fixture.manager.load(id, None).await.unwrap();
    let mut policy = fixture.manager.process_policy();
    policy.shutdown_deadline_ms += 1;
    write(
        &fixture,
        0,
        ConfigMutation::AppServer {
            authored: Some(policy),
        },
    )
    .await;
    assert_eq!(
        settled(&fixture, 0).await.units[&ApplyUnit::ProcessBindings],
        UnitApplication::ProcessRestart
    );
    let created = fixture
        .manager
        .create_session(SessionPersistentState {
            cwd: fixture.workspaces[0].clone(),
            model: None,
        })
        .await
        .unwrap()
        .session;
    assert!(
        !fixture
            .manager
            .applications
            .is_deferred(created.id.as_str())
    );
    assert!(
        fixture
            .manager
            .configuration_application(&created.id)
            .is_none()
    );
    fixture.manager.load(&created.id, None).await.unwrap();
    assert!(
        fixture
            .manager
            .configuration_application(&created.id)
            .is_none()
    );
    assert_eq!(
        fixture
            .manager
            .probe(&created.active_conversation_id)
            .configuration_preparations
            .load(Ordering::SeqCst),
        0
    );
    fixture.close().await;
}

async fn session_settled(
    fixture: &Fixture,
    id: &crate::local_runtime::session::SessionId,
) -> ConfigurationApplication {
    let mut changed = fixture.manager.configuration_changes();
    loop {
        if let Some(view) = fixture.manager.configuration_application(id)
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue385_user_commit_refreshes_desired_source_of_sessionless_workspace() {
    use crate::local_runtime::configuration::settings::SourceTarget;
    use crate::local_runtime::session::deletion::SessionDeleteResult;
    Box::pin(bounded(async {
        let fixture = Fixture::with_session_count(None, 0).await;
        let workspace = std::fs::canonicalize(&fixture.workspaces[0]).unwrap();
        write(
            &fixture,
            0,
            ConfigMutation::Instructions {
                authored: Some("available A".into()),
            },
        )
        .await;
        source_settled(&fixture, &SourceTarget::User).await;
        // Establish last-good A for the Workspace through a real Session.
        let first = fixture
            .manager
            .create_session(SessionPersistentState {
                cwd: fixture.workspaces[0].clone(),
                model: None,
            })
            .await
            .unwrap()
            .session;
        fixture.manager.load(&first.id, None).await.unwrap();
        {
            let applications = fixture.manager.applications.lock();
            assert_eq!(
                applications
                    .test_available(&workspace)
                    .unwrap()
                    .config
                    .agent
                    .instructions,
                "available A"
            );
        }
        let SessionDeleteResult::Preview { preview } =
            fixture.manager.sessions.delete_preview(&first.id).await
        else {
            panic!("preview")
        };
        let deleted = fixture
            .manager
            .delete_session(&first.id, &preview.target_revision)
            .await
            .unwrap();
        assert!(matches!(deleted, SessionDeleteResult::Deleted { .. }));
        assert!(
            fixture
                .manager
                .sessions
                .list_sessions(None, 0, 32)
                .await
                .unwrap()
                .sessions
                .is_empty()
        );
        assert!(fixture.manager.diagnostics().sessions.is_empty());
        assert!(
            fixture
                .manager
                .registry
                .0
                .lock()
                .unwrap()
                .entries
                .is_empty()
        );
        {
            let applications = fixture.manager.applications.lock();
            assert_eq!(
                applications
                    .test_available(&workspace)
                    .unwrap()
                    .config
                    .agent
                    .instructions,
                "available A"
            );
            assert!(applications.test_desired_source(&workspace).is_none());
        }
        // A User commit with zero Sessions in the known Workspace refreshes
        // only the retained desired source; it creates nothing.
        write(
            &fixture,
            0,
            ConfigMutation::Instructions {
                authored: Some("desired B".into()),
            },
        )
        .await;
        source_settled(&fixture, &SourceTarget::User).await;
        assert!(
            fixture
                .manager
                .sessions
                .list_sessions(None, 0, 32)
                .await
                .unwrap()
                .sessions
                .is_empty()
        );
        assert!(
            fixture
                .manager
                .registry
                .0
                .lock()
                .unwrap()
                .entries
                .is_empty()
        );
        let desired_revision = {
            let applications = fixture.manager.applications.lock();
            assert_eq!(
                applications
                    .test_available(&workspace)
                    .unwrap()
                    .config
                    .agent
                    .instructions,
                "available A"
            );
            let desired = applications
                .test_desired_source(&workspace)
                .expect("User commit refreshes a sessionless Workspace")
                .as_ref()
                .expect("desired capture");
            assert_eq!(
                desired.context.as_ref().unwrap().config.agent.instructions,
                "desired B"
            );
            desired.revision.clone()
        };
        // A new Session binds last-good A while the newer desired B defers to
        // natural residency; creation itself drives no preparation.
        let second = fixture
            .manager
            .create_session(SessionPersistentState {
                cwd: fixture.workspaces[0].clone(),
                model: None,
            })
            .await
            .unwrap()
            .session;
        assert_eq!(
            fixture
                .manager
                .sessions
                .configuration_bindings
                .lock()
                .unwrap()[&second.id]
                .config
                .agent
                .instructions,
            "available A"
        );
        assert!(fixture.manager.configuration_runtime(&second.id).is_none());
        assert!(fixture.manager.applications.is_deferred(second.id.as_str()));
        let pending = fixture
            .manager
            .configuration_application(&second.id)
            .unwrap();
        assert!(pending.candidate.is_none());
        assert_eq!(
            pending.desired.input_revision.as_deref(),
            Some(desired_revision.as_str())
        );
        assert!(
            pending
                .units
                .values()
                .all(|unit| matches!(unit, UnitApplication::Preparing))
        );
        fixture.manager.load(&second.id, None).await.unwrap();
        let ready = session_settled(&fixture, &second.id).await;
        let candidate = ready
            .candidate
            .expect("cache-impacting instructions candidate");
        assert_eq!(candidate.identity, ready.desired);
        assert!(matches!(
            ready.units[&ApplyUnit::Instructions],
            UnitApplication::Ready { .. }
        ));
        let adopted = fixture
            .manager
            .adopt_configuration(&second.id, &candidate.identity, candidate.expected_binding)
            .unwrap();
        assert_eq!(
            adopted.units[&ApplyUnit::Instructions],
            UnitApplication::Applied
        );
        assert_eq!(
            fixture
                .manager
                .configuration_runtime(&second.id)
                .unwrap()
                .runtime_resources()
                .configuration()
                .unwrap()
                .config
                .agent
                .instructions,
            "desired B"
        );
        {
            let applications = fixture.manager.applications.lock();
            assert_eq!(
                applications
                    .test_available(&workspace)
                    .unwrap()
                    .config
                    .agent
                    .instructions,
                "desired B"
            );
        }
        fixture.close().await;
    }))
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue385_failed_preparation_after_user_commit_keeps_last_good_available() {
    use crate::local_runtime::configuration::settings::SourceTarget;
    use crate::local_runtime::session::deletion::SessionDeleteResult;
    Box::pin(bounded(async {
        let fixture = Fixture::with_session_count(None, 0).await;
        let workspace = std::fs::canonicalize(&fixture.workspaces[0]).unwrap();
        write(
            &fixture,
            0,
            ConfigMutation::Instructions {
                authored: Some("available A".into()),
            },
        )
        .await;
        source_settled(&fixture, &SourceTarget::User).await;
        let first = fixture
            .manager
            .create_session(SessionPersistentState {
                cwd: fixture.workspaces[0].clone(),
                model: None,
            })
            .await
            .unwrap()
            .session;
        fixture.manager.load(&first.id, None).await.unwrap();
        let SessionDeleteResult::Preview { preview } =
            fixture.manager.sessions.delete_preview(&first.id).await
        else {
            panic!("preview")
        };
        fixture
            .manager
            .delete_session(&first.id, &preview.target_revision)
            .await
            .unwrap();
        assert!(
            fixture
                .manager
                .registry
                .0
                .lock()
                .unwrap()
                .entries
                .is_empty()
        );
        write(
            &fixture,
            0,
            ConfigMutation::Instructions {
                authored: Some("desired B".into()),
            },
        )
        .await;
        source_settled(&fixture, &SourceTarget::User).await;
        assert!(
            fixture
                .manager
                .registry
                .0
                .lock()
                .unwrap()
                .entries
                .is_empty()
        );
        // The new Session binds last-good A; the desired B waits for natural
        // residency, where its preparation fails.
        let second = fixture
            .manager
            .create_session(SessionPersistentState {
                cwd: fixture.workspaces[0].clone(),
                model: None,
            })
            .await
            .unwrap()
            .session;
        assert_eq!(
            fixture
                .manager
                .sessions
                .configuration_bindings
                .lock()
                .unwrap()[&second.id]
                .config
                .agent
                .instructions,
            "available A"
        );
        assert!(fixture.manager.applications.is_deferred(second.id.as_str()));
        let probe = fixture.manager.probe(&second.active_conversation_id);
        probe.fail_configuration_once.store(true, Ordering::SeqCst);
        fixture.manager.load(&second.id, None).await.unwrap();
        let failed = session_settled(&fixture, &second.id).await;
        assert!(matches!(
            failed.units[&ApplyUnit::Instructions],
            UnitApplication::Failed { .. }
        ));
        assert!(failed.candidate.is_none());
        // A remains the only available configuration; failed B is retained as
        // desired source, never silently dropped or published.
        {
            let applications = fixture.manager.applications.lock();
            assert_eq!(
                applications
                    .test_available(&workspace)
                    .unwrap()
                    .config
                    .agent
                    .instructions,
                "available A"
            );
            let desired = applications
                .test_desired_source(&workspace)
                .unwrap()
                .as_ref()
                .unwrap();
            assert_eq!(
                desired.context.as_ref().unwrap().config.agent.instructions,
                "desired B"
            );
        }
        assert_eq!(
            fixture
                .manager
                .configuration_runtime(&second.id)
                .unwrap()
                .runtime_resources()
                .configuration()
                .unwrap()
                .config
                .agent
                .instructions,
            "available A"
        );
        // A Session created after the failure still binds A, never invalid B.
        let third = fixture
            .manager
            .create_session(SessionPersistentState {
                cwd: fixture.workspaces[0].clone(),
                model: None,
            })
            .await
            .unwrap()
            .session;
        assert_eq!(
            fixture
                .manager
                .sessions
                .configuration_bindings
                .lock()
                .unwrap()[&third.id]
                .config
                .agent
                .instructions,
            "available A"
        );
        {
            let applications = fixture.manager.applications.lock();
            assert_eq!(
                applications
                    .test_available(&workspace)
                    .unwrap()
                    .config
                    .agent
                    .instructions,
                "available A"
            );
        }
        fixture.close().await;
    }))
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue385_future_default_source_generation_publishes_without_session_mutation() {
    use crate::local_runtime::configuration::settings::SourceTarget;
    use crate::local_runtime::session::deletion::SessionDeleteResult;
    Box::pin(bounded(async {
        let fixture = Fixture::with_session_count(None, 0).await;
        let workspace = std::fs::canonicalize(&fixture.workspaces[0]).unwrap();
        let model_a = crate::model::catalog::ModelRef::parse("local/a").unwrap();
        let model_b = crate::model::catalog::ModelRef::parse("local/b").unwrap();
        let selected = |model: &crate::model::catalog::ModelRef| {
            Some(crate::model::session::SessionModelConfig::of(model.clone()))
        };
        // Establish last-good A (default model local/a) through a real Session.
        let first = fixture
            .manager
            .create_session(SessionPersistentState {
                cwd: fixture.workspaces[0].clone(),
                model: None,
            })
            .await
            .unwrap()
            .session;
        fixture.manager.load(&first.id, None).await.unwrap();
        {
            let applications = fixture.manager.applications.lock();
            assert_eq!(
                applications
                    .test_available(&workspace)
                    .unwrap()
                    .config
                    .initial_model()
                    .model,
                model_a
            );
        }
        let SessionDeleteResult::Preview { preview } =
            fixture.manager.sessions.delete_preview(&first.id).await
        else {
            panic!("preview")
        };
        let deleted = fixture
            .manager
            .delete_session(&first.id, &preview.target_revision)
            .await
            .unwrap();
        assert!(matches!(deleted, SessionDeleteResult::Deleted { .. }));
        assert!(
            fixture
                .manager
                .sessions
                .list_sessions(None, 0, 32)
                .await
                .unwrap()
                .sessions
                .is_empty()
        );
        assert!(fixture.manager.diagnostics().sessions.is_empty());
        assert!(
            fixture
                .manager
                .registry
                .0
                .lock()
                .unwrap()
                .entries
                .is_empty()
        );
        // A zero-Session User commit that changes only a future-Session default
        // (local/a -> local/b) refreshes the retained desired source; it
        // creates nothing and republishes nothing by itself.
        write(
            &fixture,
            0,
            ConfigMutation::RootModel {
                authored: Some(crate::local_runtime::authoring::ModelLayer {
                    model: Some(model_b.clone()),
                    ..Default::default()
                }),
            },
        )
        .await;
        source_settled(&fixture, &SourceTarget::User).await;
        assert!(
            fixture
                .manager
                .sessions
                .list_sessions(None, 0, 32)
                .await
                .unwrap()
                .sessions
                .is_empty()
        );
        assert!(
            fixture
                .manager
                .registry
                .0
                .lock()
                .unwrap()
                .entries
                .is_empty()
        );
        let desired_revisions = {
            let applications = fixture.manager.applications.lock();
            let available = applications.test_available(&workspace).unwrap();
            assert_eq!(
                available.config.initial_model().model,
                model_a,
                "generation B waits for native publication"
            );
            let desired = applications
                .test_desired_source(&workspace)
                .expect("User commit refreshes a sessionless Workspace")
                .as_ref()
                .expect("desired capture");
            let context = desired.context.as_ref().expect("desired context");
            assert_eq!(context.config.initial_model().model, model_b);
            context.source_revisions.clone()
        };
        // A new Session binds last-good A with its own selection local/a. The
        // unpublished generation defers to natural residency; creation drives
        // no runtime, no model request, and no adoption candidate.
        let second = fixture
            .manager
            .create_session(SessionPersistentState {
                cwd: fixture.workspaces[0].clone(),
                model: None,
            })
            .await
            .unwrap()
            .session;
        assert_eq!(
            fixture
                .manager
                .sessions
                .read_settings(&second.id)
                .await
                .unwrap()
                .1
                .model,
            selected(&model_a)
        );
        assert!(fixture.manager.configuration_runtime(&second.id).is_none());
        assert!(fixture.provider.request_bodies().is_empty());
        let pending = fixture
            .manager
            .configuration_application(&second.id)
            .expect("the unpublished source generation rides this Session scope");
        assert!(pending.candidate.is_none());
        assert!(
            pending
                .units
                .values()
                .all(|unit| matches!(unit, UnitApplication::Preparing)),
            "{pending:?}"
        );
        assert!(fixture.manager.applications.is_deferred(second.id.as_str()));
        fixture.manager.load(&second.id, None).await.unwrap();
        let settled = session_settled(&fixture, &second.id).await;
        // B is session-effectively identical to S2: publication applies every
        // unit in place without a candidate and without any preparation.
        assert!(settled.candidate.is_none(), "{settled:?}");
        assert!(
            settled
                .units
                .values()
                .all(|unit| *unit == UnitApplication::Applied),
            "{settled:?}"
        );
        assert_eq!(
            fixture
                .manager
                .probe(&second.active_conversation_id)
                .configuration_preparations
                .load(Ordering::SeqCst),
            0
        );
        assert!(fixture.provider.request_bodies().is_empty());
        assert_eq!(
            fixture
                .manager
                .sessions
                .read_settings(&second.id)
                .await
                .unwrap()
                .1
                .model,
            selected(&model_a),
            "publication never mutates the resident Session's selection"
        );
        {
            let applications = fixture.manager.applications.lock();
            let available = applications.test_available(&workspace).unwrap();
            assert_eq!(
                available.source_revisions, desired_revisions,
                "the joining Session publishes generation B"
            );
            assert_eq!(available.config.initial_model().model, model_b);
        }
        // Sessions created after publication bind the new default; the resident
        // Session keeps its own selection and registers no new work.
        let third = fixture
            .manager
            .create_session(SessionPersistentState {
                cwd: fixture.workspaces[0].clone(),
                model: None,
            })
            .await
            .unwrap()
            .session;
        assert_eq!(
            fixture
                .manager
                .sessions
                .read_settings(&third.id)
                .await
                .unwrap()
                .1
                .model,
            selected(&model_b)
        );
        assert!(!fixture.manager.applications.is_deferred(third.id.as_str()));
        assert_eq!(
            fixture
                .manager
                .sessions
                .read_settings(&second.id)
                .await
                .unwrap()
                .1
                .model,
            selected(&model_a)
        );
        fixture.close().await;
    }))
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue385_failed_future_default_publication_keeps_last_good_available() {
    use crate::local_runtime::configuration::settings::SourceTarget;
    use crate::local_runtime::session::deletion::SessionDeleteResult;
    Box::pin(bounded(async {
        let fixture = Fixture::with_session_count(None, 0).await;
        let workspace = std::fs::canonicalize(&fixture.workspaces[0]).unwrap();
        let model_a = crate::model::catalog::ModelRef::parse("local/a").unwrap();
        let model_b = crate::model::catalog::ModelRef::parse("local/b").unwrap();
        let selected = |model: &crate::model::catalog::ModelRef| {
            Some(crate::model::session::SessionModelConfig::of(model.clone()))
        };
        let first = fixture
            .manager
            .create_session(SessionPersistentState {
                cwd: fixture.workspaces[0].clone(),
                model: None,
            })
            .await
            .unwrap()
            .session;
        fixture.manager.load(&first.id, None).await.unwrap();
        let SessionDeleteResult::Preview { preview } =
            fixture.manager.sessions.delete_preview(&first.id).await
        else {
            panic!("preview")
        };
        fixture
            .manager
            .delete_session(&first.id, &preview.target_revision)
            .await
            .unwrap();
        assert!(
            fixture
                .manager
                .registry
                .0
                .lock()
                .unwrap()
                .entries
                .is_empty()
        );
        write(
            &fixture,
            0,
            ConfigMutation::RootModel {
                authored: Some(crate::local_runtime::authoring::ModelLayer {
                    model: Some(model_b.clone()),
                    ..Default::default()
                }),
            },
        )
        .await;
        source_settled(&fixture, &SourceTarget::User).await;
        // The joining Session binds last-good A while the unpublished
        // generation B defers to natural residency.
        let second = fixture
            .manager
            .create_session(SessionPersistentState {
                cwd: fixture.workspaces[0].clone(),
                model: None,
            })
            .await
            .unwrap()
            .session;
        assert_eq!(
            fixture
                .manager
                .sessions
                .read_settings(&second.id)
                .await
                .unwrap()
                .1
                .model,
            selected(&model_a)
        );
        let pending = fixture
            .manager
            .configuration_application(&second.id)
            .expect("the unpublished source generation rides this Session scope");
        assert!(pending.candidate.is_none());
        assert!(fixture.manager.applications.is_deferred(second.id.as_str()));
        // The Workspace source breaks before publication; the deferred capture
        // fails and last-good A remains the only available configuration.
        std::fs::write(fixture.workspaces[0].join("rustx.toml"), "broken = [").unwrap();
        fixture
            .manager
            .reconcile_configuration(&SourceTarget::Workspace {
                directory: fixture.workspaces[0].clone(),
            })
            .await
            .unwrap();
        let failed = session_settled(&fixture, &second.id).await;
        assert!(
            failed
                .units
                .values()
                .any(|unit| matches!(unit, UnitApplication::Failed { .. })),
            "{failed:?}"
        );
        assert!(failed.candidate.is_none());
        {
            let applications = fixture.manager.applications.lock();
            assert_eq!(
                applications
                    .test_available(&workspace)
                    .unwrap()
                    .config
                    .initial_model()
                    .model,
                model_a
            );
            assert!(
                applications
                    .test_desired_source(&workspace)
                    .unwrap()
                    .is_err(),
                "the broken generation is retained as the failed desired source"
            );
        }
        // A Session created before any retry still binds last-good A.
        let third = fixture
            .manager
            .create_session(SessionPersistentState {
                cwd: fixture.workspaces[0].clone(),
                model: None,
            })
            .await
            .unwrap()
            .session;
        assert_eq!(
            fixture
                .manager
                .sessions
                .read_settings(&third.id)
                .await
                .unwrap()
                .1
                .model,
            selected(&model_a)
        );
        // Repairing the source retries through the existing reconcile path; the
        // retained generation publishes without any Session residency.
        std::fs::write(
            fixture.workspaces[0].join("rustx.toml"),
            "[environment]\nRESIDENCY_SESSION = \"A\"\n",
        )
        .unwrap();
        fixture
            .manager
            .reconcile_configuration(&SourceTarget::Workspace {
                directory: fixture.workspaces[0].clone(),
            })
            .await
            .unwrap();
        let mut changes = fixture.manager.configuration_changes();
        loop {
            {
                let applications = fixture.manager.applications.lock();
                if applications
                    .test_available(&workspace)
                    .is_some_and(|available| available.config.initial_model().model == model_b)
                {
                    break;
                }
            }
            changes.changed().await.unwrap();
        }
        let fourth = fixture
            .manager
            .create_session(SessionPersistentState {
                cwd: fixture.workspaces[0].clone(),
                model: None,
            })
            .await
            .unwrap()
            .session;
        assert_eq!(
            fixture
                .manager
                .sessions
                .read_settings(&fourth.id)
                .await
                .unwrap()
                .1
                .model,
            selected(&model_b)
        );
        assert_eq!(
            fixture
                .manager
                .sessions
                .read_settings(&second.id)
                .await
                .unwrap()
                .1
                .model,
            selected(&model_a)
        );
        fixture.close().await;
    }))
    .await;
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn native_source_application_identity_uses_exact_facts_not_projection() {
    use crate::local_runtime::configuration::{UserConfigManager, settings::SourceTarget};
    use std::{ffi::OsString, os::unix::ffi::OsStringExt};

    let mut f = Fixture::with_session_count(None, 0).await;
    let mut sources = f.manager.configuration.source_bindings().clone();
    let bytes = std::fs::read(&sources.config_path).unwrap();
    let parent = sources.config_path.parent().unwrap();
    let a = parent.join(OsString::from_vec(b"source-\xff.toml".to_vec()));
    let b = parent.join(OsString::from_vec(b"source-\xfe.toml".to_vec()));
    assert_eq!(a.to_string_lossy(), b.to_string_lossy());
    for path in [&a, &b] {
        std::fs::write(path, &bytes).unwrap();
    }
    sources.config_path = a.clone();
    f.manager.configuration = UserConfigManager::new(sources.clone()).unwrap();
    let target = SourceTarget::User;
    let projection = f
        .manager
        .configuration
        .read_source_settings(&target)
        .unwrap();
    assert_eq!(projection.user.path, a);
    assert!(projection.user.authored.is_some());
    assert!(serde_json::to_vec(&projection).is_err());
    let capture = |manager: &SessionRuntimeManager| {
        let mut state = manager.applications.lock();
        manager.capture_source_consumers(&mut state, &target);
        state
            .view(&target.application_scope())
            .unwrap()
            .desired
            .input_revision
            .unwrap()
    };
    let original = capture(&f.manager);
    assert_eq!(original, capture(&f.manager));

    // The wire view changes, but an immutable process location is not an
    // authored source input. Re-hashing SourceSettings would fail this contract.
    sources.runtime_root = sources.runtime_root.join("other-binding");
    f.manager.configuration = UserConfigManager::new(sources.clone()).unwrap();
    let other = f
        .manager
        .configuration
        .read_source_settings(&target)
        .unwrap();
    assert_ne!(projection.runtime_root, other.runtime_root);
    assert_eq!(original, capture(&f.manager));

    let mut changed = bytes.clone();
    changed.extend_from_slice(b"\n# changed authored bytes\n");
    std::fs::write(&a, changed).unwrap();
    assert_ne!(original, capture(&f.manager));
    std::fs::write(&a, &bytes).unwrap();
    assert_eq!(original, capture(&f.manager));
    sources.config_path = b;
    f.manager.configuration = UserConfigManager::new(sources).unwrap();
    assert_ne!(original, capture(&f.manager));
    let root = f.manager.configuration.resource_root(&target);
    std::fs::create_dir_all(&root).unwrap();
    let before_resource_change = capture(&f.manager);
    std::fs::write(root.join("AGENTS.md"), "new native instructions").unwrap();
    assert_ne!(before_resource_change, capture(&f.manager));
    assert!(f.manager.registry.0.lock().unwrap().entries.is_empty());
    assert!(
        f.manager
            .sessions
            .list_sessions(None, 0, 32)
            .await
            .unwrap()
            .sessions
            .is_empty()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue402_workspace_session_models_are_the_created_session_catalog() {
    use crate::local_runtime::configuration::settings::{SessionModelsView, SourceTarget};
    Box::pin(bounded(async {
        let fixture = Fixture::new().await;
        let directory = fixture.workspaces[0].clone();
        let workspace = SourceTarget::Workspace {
            directory: directory.clone(),
        };
        let user = fixture
            .manager
            .source_settings(&SourceTarget::User, None)
            .await
            .unwrap();
        assert_eq!(user.session_models, None);

        let create = |cwd: std::path::PathBuf| {
            fixture
                .manager
                .create_session(SessionPersistentState { cwd, model: None })
        };
        // The first creation publishes this Workspace's binding. A model then
        // authored in its document but never published is configuration, not
        // a selectable model of a Session created now.
        create(directory.clone()).await.unwrap();
        let authored = std::fs::read_to_string(&user.user.path).unwrap();
        let document: toml::Value = toml::from_str(&authored).unwrap();
        let mut unpublished = document["models"]["local/a"].clone();
        unpublished["id"] = "unpublished".into();
        let mut layer = toml::Table::new();
        layer.insert(
            "models".into(),
            toml::Value::Table(toml::Table::from_iter([(
                "local/unpublished".into(),
                unpublished,
            )])),
        );
        std::fs::write(
            directory.join("rustx.toml"),
            format!(
                "[environment]\nRESIDENCY_SESSION = \"A\"\n{}",
                toml::to_string(&layer).unwrap()
            ),
        )
        .unwrap();
        let source = fixture
            .manager
            .source_settings(&workspace, None)
            .await
            .unwrap();
        assert!(
            source
                .resolved
                .as_ref()
                .unwrap()
                .models
                .as_ref()
                .unwrap()
                .contains_key("local/unpublished")
        );
        let Some(SessionModelsView::Available {
            catalog,
            default_model,
        }) = source.session_models
        else {
            panic!("Workspace Session catalog: {:?}", source.session_models)
        };
        let listed: Vec<_> = catalog
            .models
            .iter()
            .map(|model| model.model.to_string())
            .collect();
        assert_eq!(listed, ["local/a", "local/b"]);
        assert_eq!(default_model.model.to_string(), "local/a");

        let created = create(directory).await.unwrap().session.id;
        fixture.manager.load(&created, None).await.unwrap();
        assert_eq!(
            fixture
                .manager
                .configuration_runtime(&created)
                .unwrap()
                .model_catalog(),
            catalog
        );

        // Where native cannot bind a Session, there is no catalog to choose
        // from, and the read creates no Session.
        let broken = fixture.workspaces[0].parent().unwrap().join("broken");
        std::fs::create_dir(&broken).unwrap();
        std::fs::write(
            broken.join("rustx.toml"),
            "[agent.model]\nmodel = \"local/missing\"\n",
        )
        .unwrap();
        let count = || async {
            fixture
                .manager
                .sessions
                .list_sessions(None, 0, 32)
                .await
                .unwrap()
                .sessions
                .len()
        };
        let before = count().await;
        let source = fixture
            .manager
            .source_settings(
                &SourceTarget::Workspace {
                    directory: broken.clone(),
                },
                None,
            )
            .await
            .unwrap();
        let Some(SessionModelsView::Unavailable { diagnostic }) = source.session_models else {
            panic!(
                "broken Workspace Session catalog: {:?}",
                source.session_models
            )
        };
        assert!(diagnostic.contains("local/missing"), "{diagnostic}");
        assert_eq!(count().await, before);
        assert!(create(broken).await.is_err());
        fixture.close().await;
    }))
    .await;
}
