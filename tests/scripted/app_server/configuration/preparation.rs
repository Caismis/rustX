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
    set_file_guidance(&fixture, "pending context");
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
    let application = applied(&fixture, 0).await;
    assert_eq!(
        application.units[&ApplyUnit::ExecutionPolicy],
        UnitApplication::Applied
    );
    assert_eq!(
        application.units[&ApplyUnit::Instructions],
        UnitApplication::Applied
    );
    assert_eq!(
        application.units[&ApplyUnit::ProcessBindings],
        UnitApplication::ProcessRestart
    );
    assert_eq!(fixture.host.policy().max_connections, 9);
    assert_eq!(fixture.host.policy().shutdown_deadline_ms, 30_000);
    let runtime = fixture.manager.configuration_runtime(id).unwrap();
    assert_eq!(
        crate::local_runtime::configuration::settings::EffectiveConfiguration::adopted(&runtime)
            .unwrap()
            .approval_mode,
        crate::runtime::ApprovalMode::FullAccess
    );
    assert_eq!(
        guidance(runtime.runtime_resources().project_context_files()),
        "pending context"
    );
    fixture.close().await;
}

#[tokio::test]
async fn t05_complete_policy_registry_publishes_independently_of_instructions() {
    let fixture = Fixture::with_session_count(None, 1).await;
    let id = &fixture.sessions[0].id;
    fixture.manager.load(id, None).await.unwrap();
    let runtime = fixture.manager.configuration_runtime(id).unwrap();
    let old = runtime.runtime_resources();
    write_guidance(&fixture, "pending instructions".into()).await;
    applied(&fixture, 0).await;
    let instructions = runtime.runtime_resources();
    write(
        &fixture,
        0,
        ConfigMutation::NativeTools {
            authored: Some(vec!["read".into(), "grep".into()]),
        },
    )
    .await;
    let application = applied(&fixture, 0).await;
    assert_eq!(
        application.units[&ApplyUnit::Capabilities],
        UnitApplication::Applied,
        "{application:?}"
    );
    let current = runtime.runtime_resources();
    assert_eq!(
        guidance(current.project_context_files()),
        "pending instructions"
    );
    assert_eq!(
        guidance(instructions.project_context_files()),
        guidance(current.project_context_files())
    );
    let names = |resources: &crate::runtime::RuntimeResourceSnapshot| {
        resources
            .capability()
            .tool_registry()
            .definitions()
            .into_iter()
            .map(|tool| tool.name)
            .collect::<std::collections::BTreeSet<_>>()
    };
    assert!(!names(&old).contains("grep"));
    assert!(names(&current).contains("grep"));
    assert!(
        crate::local_runtime::configuration::settings::EffectiveConfiguration::adopted(&runtime)
            .unwrap()
            .available_tools
            .iter()
            .any(|tool| tool.name == "grep")
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
        write_guidance(&fixture, "prepared for old model".into()).await;
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
        // The stale preparation never commits; the recaptured input commits
        // over the new baseline and keeps the explicit selection.
        let application = applied(&fixture, 0).await;
        assert_eq!(
            application.units[&ApplyUnit::Instructions],
            UnitApplication::Applied
        );
        assert_eq!(runtime.model_view().configured.model.to_string(), "local/b");
        assert_eq!(
            guidance(runtime.runtime_resources().project_context_files()),
            "prepared for old model"
        );
        assert_eq!(configuration_changes(&runtime).len(), 1);
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
                    source.global_guidance.unwrap().revision,
                    SourceMutation::GlobalGuidance {
                        document: "survives disconnect".into(),
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
        probe.before_configuration_prepare.release();
        let ready = applied(&fixture, 0).await;
        assert_eq!(
            ready.units[&ApplyUnit::Instructions],
            UnitApplication::Applied
        );
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
            ConfigMutation::NativeTools {
                authored: Some(vec!["read".into(), "grep".into()]),
            },
        )
        .await;
        // The admitted Attempt owns its registry, so the capability change is staged.
        let staged = settled(&fixture, 0).await;
        assert_eq!(
            staged.units[&ApplyUnit::Capabilities],
            UnitApplication::Ready
        );
        assert_eq!(runtime.runtime_resources().revision(), captured.generation);
        let mut settlement = SettlementReceiptWait::capture(&runtime);
        // Only now can the provider release the Tool proposal. The Tool batch
        // and subsequent model Step are created after newer selection was saved.
        fixture.gates[0].release();
        settlement.wait().await;
        let committed = applied(&fixture, 0).await;
        assert_eq!(
            committed.units[&ApplyUnit::Capabilities],
            UnitApplication::Applied
        );
        assert!(runtime.runtime_resources().revision() > captured.generation);
        let requests = fixture.provider.request_bodies();
        assert_eq!(
            requests.len(),
            2,
            "admitted registry executes the Tool and reaches the next Step"
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
    assert!(
        applied
            .units
            .values()
            .all(|unit| *unit != UnitApplication::Ready),
        "{applied:?}"
    );
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
            .authored
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
        write_guidance(&fixture, "retry this input".into()).await;
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
        let retried = applied(&fixture, 0).await;
        assert_eq!(
            retried.desired.input_revision,
            failed.desired.input_revision
        );
        assert!(retried.desired.attempt > failed.desired.attempt);
        assert_eq!(
            retried.units[&ApplyUnit::Instructions],
            UnitApplication::Applied
        );
        let before = probe.configuration_preparations.load(Ordering::SeqCst);
        probe.before_configuration_prepare.arm();
        write_guidance(&fixture, "superseded".into()).await;
        probe.before_configuration_prepare.entered().await;
        for n in 0..12 {
            write_guidance(&fixture, format!("latest {n}")).await;
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
        let ready = applied(&fixture, 0).await;
        assert_eq!(ready.desired, latest);
        assert_eq!(
            probe.configuration_preparations.load(Ordering::SeqCst),
            before + 2,
            "only the latest pending input is prepared after old work settles"
        );
        assert_eq!(
            guidance(
                fixture
                    .manager
                    .configuration_runtime(id)
                    .unwrap()
                    .runtime_resources()
                    .project_context_files()
            ),
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

#[tokio::test]
async fn t03_t05_failed_capabilities_preserve_leases_while_instructions_commit() {
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
    initial["agent"].as_table_mut().unwrap().insert(
        "agents_md".into(),
        toml::Value::try_from(serde_json::json!({"inherit": false})).unwrap(),
    );
    set_file_guidance(&fixture, "User I1");
    std::fs::write(
        &initial_source.user.path,
        toml::to_string(&initial).unwrap(),
    )
    .unwrap();
    std::fs::write(
        fixture.workspaces[0].join("AGENTS.md"),
        "Workspace I1 project input",
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
    let project_file = fixture.workspaces[0].join("AGENTS.md");
    std::fs::write(&project_file, "Workspace I2 project input").unwrap();
    set_file_guidance(&fixture, "independent instructions P2");
    std::fs::write(
        fixture.workspaces[0].join("rustx.toml"),
        "approval_mode='full_access'\n[subagents]\nmax_concurrent=3\n[agent.agents_md]\ninherit=true\n",
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
        ConfigMutation::NativeTools {
            authored: Some(vec!["read".into(), "grep".into()]),
        },
    )
    .await;
    let application = applied(&fixture, 0).await;
    assert!(
        matches!(
            application.units[&ApplyUnit::Capabilities],
            UnitApplication::Failed { .. }
        ),
        "{application:?}"
    );
    assert_eq!(
        application.units[&ApplyUnit::Instructions],
        UnitApplication::Applied,
        "independent instructions commit while the failed closure keeps its leases"
    );
    let after = runtime.runtime_resources();
    assert_eq!(
        guidance(after.project_context_files()),
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
        before.configuration().unwrap().authored.component_revisions[&ApplyUnit::Capabilities],
        after.configuration().unwrap().authored.component_revisions[&ApplyUnit::Capabilities]
    );
    assert_ne!(
        before.configuration().unwrap().authored.component_revisions[&ApplyUnit::Instructions],
        after.configuration().unwrap().authored.component_revisions[&ApplyUnit::Instructions]
    );
    assert!(Arc::ptr_eq(
        before.capability().tool_registry(),
        after.capability().tool_registry()
    ));
    assert_eq!(
        before.configuration().unwrap().config.agent.tools.builtin,
        after.configuration().unwrap().config.agent.tools.builtin
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
    assert!(c1i2.config.agent.agents_md.inherit);
    assert_eq!(c1i2.project_context_files, after.project_context_files());
    assert!(matches!(
        c1i2.provenance["agent.agents_md"],
        crate::local_runtime::configuration::Origin::Workspace { .. }
    ));
    c1i2.validate_resource_authority().unwrap();
    assert!(
        c1i2.project_context_files
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
            after.configuration().unwrap().authored.component_revisions[&unit]
        );
    }

    assert!(fixture.manager.applications.is_deferred(second.as_str()));
    assert!(matches!(
        before.configuration().unwrap().authored.provenance["agent.agents_md"],
        crate::local_runtime::configuration::Origin::User { .. }
    ));
    assert!(matches!(
        c1i2.provenance["agent.agents_md"],
        crate::local_runtime::configuration::Origin::Workspace { .. }
    ));
    assert!(matches!(
        after.configuration().unwrap().authored.provenance["agent.agents_md"],
        crate::local_runtime::configuration::Origin::Workspace { .. }
    ));
    assert_eq!(
        c1i2.component_revisions[&ApplyUnit::Instructions],
        after.configuration().unwrap().authored.component_revisions[&ApplyUnit::Instructions]
    );
    assert_eq!(
        c1i2.source_revisions,
        before.configuration().unwrap().authored.source_revisions
    );
    assert_eq!(
        after.configuration().unwrap().authored.source_revisions,
        before.configuration().unwrap().authored.source_revisions
    );
    assert_eq!(
        guidance(&c1i2.project_context_files),
        "independent instructions P2"
    );
    assert_eq!(
        c1i2.component_revisions[&ApplyUnit::Capabilities],
        before.configuration().unwrap().authored.component_revisions[&ApplyUnit::Capabilities]
    );
    assert_eq!(
        c1i2.config.agent.tools.builtin,
        before.configuration().unwrap().config.agent.tools.builtin
    );
    assert_eq!(
        c1i2.skill_discovery.packages,
        retained(id).skill_discovery.packages
    );
    // The second Session composes from its retained C1 closure; its deferred
    // C2 is then prepared again for that allocation and commits as one
    // recorded reallocation whose `before` is exactly the retained C1 Tools.
    assert_eq!(
        retained(&second).config.agent.tools.builtin,
        before.configuration().unwrap().config.agent.tools.builtin
    );
    fixture.manager.load(&second, None).await.unwrap();
    session_applied(&fixture, &second).await;
    let second_runtime = fixture.manager.configuration_runtime(&second).unwrap();
    let changes = configuration_changes(&second_runtime);
    let [change] = changes.as_slice() else {
        panic!("one reallocation fact: {changes:?}");
    };
    assert_eq!(
        change.reason,
        crate::runtime::configuration_commit::ConfigurationChangeReason::Reallocation
    );
    assert!(
        change.tools.added.names.contains(&"grep".to_owned()),
        "{change:?}"
    );
    let catalog = configuration_catalogs(&second_runtime.tool_runtime().durable_store())
        .pop()
        .unwrap();
    let tools = catalog["tools"].as_array().unwrap();
    assert!(tools.iter().any(|tool| tool["name"] == "grep"), "{catalog}");
    // Retry the same authored C2: it prepares and every resident Session
    // commits it automatically, while new Sessions bind C2 as well.
    fixture
        .manager
        .reconcile_configuration(
            &crate::local_runtime::configuration::settings::SourceTarget::Workspace {
                directory: fixture.workspaces[0].clone(),
            },
        )
        .await
        .unwrap();
    applied(&fixture, 0).await;
    session_applied(&fixture, &second).await;
    let third = create().await.unwrap().session.id;
    let c2i2 = retained(&third);
    assert_eq!(
        guidance(&c2i2.project_context_files),
        "independent instructions P2"
    );
    assert_ne!(
        c2i2.component_revisions[&ApplyUnit::Capabilities],
        c1i2.component_revisions[&ApplyUnit::Capabilities]
    );
    assert_ne!(
        c2i2.config.agent.tools.builtin,
        c1i2.config.agent.tools.builtin
    );
    assert_eq!(
        fixture
            .manager
            .configuration_runtime(&second)
            .unwrap()
            .runtime_resources()
            .configuration()
            .unwrap()
            .config
            .agent
            .tools
            .builtin,
        c2i2.config.agent.tools.builtin
    );
    assert_eq!(
        retained(&second).component_revisions[&ApplyUnit::Capabilities],
        c2i2.component_revisions[&ApplyUnit::Capabilities]
    );
    assert_eq!(
        guidance(runtime.runtime_resources().project_context_files()),
        "independent instructions P2"
    );
    assert_ne!(
        runtime.runtime_resources().capability().revision(),
        before.capability().revision()
    );
    assert!(!before.capability().skills().packages().is_empty());
    fixture.close().await;
}
