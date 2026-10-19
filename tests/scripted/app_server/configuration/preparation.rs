//! Preparation and publication of mixed applications: process binding,
//! policy registries, model baselines, lost write responses, bounded retries
//! and capability failures that preserve leases.
#![allow(clippy::large_futures)] // bounded fixture futures; no recursive or unbounded stack growth
use super::*;

#[tokio::test]
async fn t03_mixed_application_and_true_process_binding_restart() {
    let fixture = Fixture::with_session_count(None, 1).await;
    let id = &fixture.sessions[0].id;
    fixture.manager.load(id, None).await.unwrap();
    let source = fixture
        .manager
        .source_settings(&crate::local_runtime::configuration::settings::SourceTarget::User)
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
    let fixture = Fixture::with_session_count(None, 1).await;
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
async fn t08_model_baseline_change_recaptures_prepared_context() {
    bounded(async {
        let fixture = Fixture::with_session_count(None, 1).await;
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
        fixture
            .manager
            .set_model(
                id,
                crate::model::session::SessionModelConfig::of(
                    crate::model::catalog::ModelRef::parse("local/b").unwrap(),
                ),
            )
            .await
            .unwrap();
        probe.before_configuration_publish.release();
        let application = settled(&fixture, 0).await;
        let recaptured = application
            .candidate
            .as_ref()
            .expect("new baseline becomes ready");
        assert_eq!(recaptured.identity, application.desired);
        assert_eq!(
            recaptured.expected_binding,
            runtime.configuration_view().unwrap().adopted_binding
        );
        assert!(matches!(
            application.units[&ApplyUnit::Instructions],
            UnitApplication::Ready { .. }
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
        assert_ne!(
            application.desired, candidate_id,
            "stale preparation is superseded"
        );
        fixture.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t12_lost_source_write_response_does_not_cancel_native_application() {
    bounded(async {
        let fixture = Fixture::with_session_count(None, 1).await;
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
            .source_settings(&crate::local_runtime::configuration::settings::SourceTarget::User)
            .await
            .unwrap();
        let manager = fixture.manager.clone();
        let target = crate::local_runtime::configuration::settings::SourceTarget::User;
        let rpc = tokio::spawn(async move {
            manager
                .write_source_settings(
                    &target,
                    source.user.revision,
                    SourceMutation::Config {
                        mutation: ConfigMutation::Instructions {
                            authored: Some("survives disconnect".into()),
                        },
                    },
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
            .source_settings(&crate::local_runtime::configuration::settings::SourceTarget::User)
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
        let fixture = Fixture::with_session_count(Some("read"), 1).await;
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
        let mut settlement = SettlementReceiptWait::capture(&runtime);
        // Only now can the provider release the Tool proposal. The Tool batch
        // and subsequent model Step are created after newer policy published.
        fixture.gates[0].release();
        settlement.wait().await;
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
async fn t09_unselected_model_and_default_edits_publish_without_adoption_t15_new_default() {
    let fixture = Fixture::with_session_count(None, 1).await;
    let id = &fixture.sessions[0].id;
    fixture.manager.load(id, None).await.unwrap();
    let runtime = fixture.manager.configuration_runtime(id).unwrap();
    let old = runtime.runtime_resources();
    let invocation = runtime.model_snapshot();
    let source = fixture
        .manager
        .source_settings(&crate::local_runtime::configuration::settings::SourceTarget::User)
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
    // The redefined unselected Model is published to this Session's catalog;
    // its own invocation and every resource are retained, and the new
    // default is not this Session's selection.
    let published = runtime.runtime_resources();
    assert_eq!(published.revision(), old.revision().next());
    assert!(Arc::ptr_eq(old.capability(), published.capability()));
    assert_eq!(runtime.model_snapshot(), invocation);
    assert_eq!(
        published
            .configuration()
            .unwrap()
            .effective
            .models
            .as_ref()
            .unwrap()["local/b"]
            .id,
        "b-redefined"
    );
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
        let fixture = Fixture::with_session_count(None, 1).await;
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
    let fixture = Fixture::with_session_count(None, 1).await;
    let id = &fixture.sessions[0].id;
    fixture.manager.load(id, None).await.unwrap();
    let directory = fixture.workspaces[0].join(".agents");
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(directory.join("unrelated.txt"), "resource manifest entry").unwrap();
    let selected = crate::model::session::SessionModelConfig::of(
        crate::model::catalog::ModelRef::parse("local/b").unwrap(),
    );
    assert!(fixture.manager.configuration_application(id).is_none());
    let view = fixture
        .manager
        .set_model(id, selected.clone())
        .await
        .unwrap()
        .0;
    assert_eq!(view.configured, selected);
    let runtime = fixture.manager.configuration_runtime(id).unwrap();
    let before = runtime.runtime_resources();
    assert!(
        fixture.manager.configuration_application(id).is_none(),
        "selection must not ingest sources or manufacture a configuration application"
    );
    fixture.manager.set_model(id, selected).await.unwrap();
    assert!(Arc::ptr_eq(&before, &runtime.runtime_resources()));
    assert!(fixture.manager.configuration_application(id).is_none());
    fixture.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t03_t05_mixed_instructions_publication_rejects_rebound_project_path() {
    bounded(async {
        let fixture = Fixture::with_session_count(None, 1).await;
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
    let fixture = Fixture::with_session_count(None, 1).await;
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
        .source_settings(&crate::local_runtime::configuration::settings::SourceTarget::User)
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
        .source_settings(&crate::local_runtime::configuration::settings::SourceTarget::User)
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
