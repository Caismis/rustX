//! Session creation bindings: exact native application identity, the created
//! Session's model catalog, initial model commits and multi-client visibility.
#![allow(clippy::large_futures)] // bounded fixture futures; no recursive or unbounded stack growth
use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cold_bound_load_rechecks_user_revision_at_retained_publication() {
    use crate::local_runtime::configuration::settings::SourceTarget;
    Box::pin(bounded(async {
        let fixture = Fixture::with_session_count(None, 1).await;
        let mut client = fixture.start_execution(0).await;
        let (entered, capture_ready) = tokio::sync::oneshot::channel();
        let (release, released) = std::sync::mpsc::channel();
        fixture.manager.configuration.test_hooks.insert(
            "before_bound_initial_publication",
            move || {
                entered.send(()).unwrap();
                released.recv().unwrap();
            },
        );
        let loading = fixture.load(0);
        capture_ready.await.unwrap();
        let path = &fixture.manager.configuration.source_bindings().config_path;
        let mut document: toml::Table =
            toml::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        document.insert(
            "tool_deadline_policy".into(),
            toml::Value::Table(toml::from_str("hard_deadline_ms = 23456").unwrap()),
        );
        std::fs::write(path, toml::to_string(&document).unwrap()).unwrap();
        fixture
            .manager
            .reconcile_configuration(&SourceTarget::User)
            .await
            .unwrap();
        release.send(()).unwrap();
        loading.await.unwrap().unwrap();
        {
            let bindings = fixture
                .manager
                .sessions
                .configuration_bindings
                .lock()
                .unwrap();
            let binding = &bindings[&fixture.sessions[0].id];
            assert_eq!(binding.config.tool_deadline_policy.hard_deadline_ms, 23_456);
        }
        client.kill().await.unwrap();
    }))
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn blocked_bound_capture_leaves_other_source_transactions_available() {
    use crate::local_runtime::configuration::settings::SourceTarget;
    Box::pin(bounded(async {
        let fixture = Fixture::with_session_count(None, 2).await;
        let mut client = fixture.start_execution(0).await;
        fixture.load(0).await.unwrap().unwrap();
        let endpoint = fixture
            .manager
            .execution_endpoint(&fixture.sessions[0].id)
            .unwrap();
        let (parked, parked_receipt) = tokio::sync::oneshot::channel();
        let (release, released) = std::sync::mpsc::channel();
        let exchange = tokio::task::spawn_blocking(move || {
            endpoint.hold_data_exchange_for_test(|| {
                parked.send(()).unwrap();
                released.recv().unwrap();
            })
        });
        parked_receipt.await.unwrap();
        let (entered, reading) = tokio::sync::oneshot::channel();
        fixture
            .manager
            .configuration
            .test_hooks
            .insert("before_bound_source_read", move || {
                entered.send(()).unwrap();
            });
        let manager = fixture.manager.clone();
        let workspace = fixture.workspaces[0].clone();
        std::fs::write(
            workspace.join("rustx.toml"),
            "[tool_deadline_policy]\nhard_deadline_ms = 34567\n",
        )
        .unwrap();
        let reconciliation = tokio::spawn(async move {
            manager
                .reconcile_configuration(&SourceTarget::Workspace {
                    directory: workspace,
                })
                .await
        });
        reading.await.unwrap();
        let unrelated = SourceTarget::Workspace {
            directory: fixture.workspaces[1].clone(),
        };
        fixture.manager.source_settings(&unrelated).await.unwrap();
        fixture
            .manager
            .reconcile_configuration(&unrelated)
            .await
            .unwrap();
        assert!(!reconciliation.is_finished());
        release.send(()).unwrap();
        exchange.await.unwrap().unwrap();
        reconciliation.await.unwrap().unwrap();
        {
            let state = fixture.manager.applications.lock();
            let applied = state.view(fixture.sessions[0].id.as_ref()).unwrap();
            assert!(applied.desired.input_revision.is_some(), "{applied:?}");
        }
        client.kill().await.unwrap();
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
        assert!(
            manager
                .capture_source_consumers(&mut state, &target)
                .is_empty()
        );
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
    assert_eq!(
        f.manager
            .sessions
            .list_sessions(None, 0, 32)
            .await
            .unwrap()
            .sessions,
        [] as [crate::local_runtime::session::SessionSummary; 0]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue402_workspace_session_models_are_the_created_session_catalog() {
    use crate::local_runtime::configuration::settings::{SessionModelsView, SourceTarget};
    Box::pin(bounded(async {
        let fixture = Fixture::with_session_count(None, 0).await;
        let directory = fixture.workspaces[0].clone();
        let workspace = SourceTarget::Workspace {
            directory: directory.clone(),
        };
        let user = fixture
            .manager
            .source_settings(&SourceTarget::User)
            .await
            .unwrap();
        assert_eq!(user.session_models, None);

        let create = |cwd: std::path::PathBuf| {
            fixture.manager.create_session(SessionPersistentState {
                cwd,
                model: None,
                session_config: None,
                session_resources: None,
            })
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
        let source = fixture.manager.source_settings(&workspace).await.unwrap();
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

        // A missing default model can fall back to a selectable model without
        // altering the authored preference or creating a Session on a read.
        let removed = fixture.workspaces[0].parent().unwrap().join("removed");
        std::fs::create_dir(&removed).unwrap();
        std::fs::write(
            removed.join("rustx.toml"),
            "[agent.model]\nmodel = \"local/missing\"\n",
        )
        .unwrap();
        let fallback = fixture
            .manager
            .source_settings(&SourceTarget::Workspace { directory: removed })
            .await
            .unwrap();
        assert!(
            matches!(fallback.session_models, Some(SessionModelsView::Available {
            default_model, ..
        }) if default_model.model.as_str() == "local/a")
        );

        // An invalid preset for an existing model is still a configuration
        // failure. A read creates no Session where native cannot bind one.
        let broken = fixture.workspaces[0].parent().unwrap().join("broken");
        std::fs::create_dir(&broken).unwrap();
        std::fs::write(
            broken.join("rustx.toml"),
            "[agent.model]\nmodel = \"local/a\"\nprofile = \"missing-profile\"\n",
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
            .source_settings(&SourceTarget::Workspace {
                directory: broken.clone(),
            })
            .await
            .unwrap();
        let Some(SessionModelsView::Unavailable { diagnostic }) = source.session_models else {
            panic!(
                "broken Workspace Session catalog: {:?}",
                source.session_models
            )
        };
        assert!(diagnostic.contains("missing-profile"), "{diagnostic}");
        assert_eq!(count().await, before);
        assert!(create(broken).await.is_err());
        fixture.close().await;
    }))
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn deleting_a_model_releases_creation_when_a_named_agent_depends_on_it() {
    use crate::local_runtime::configuration::settings::{SessionModelsView, SourceTarget};
    use crate::runtime::subagent::SubagentName;
    Box::pin(bounded(async {
        let fixture = Fixture::with_session_count(None, 0).await;
        let directory = fixture.workspaces[0].clone();
        let target = SourceTarget::Workspace {
            directory: directory.clone(),
        };
        let agent = SubagentName::parse("reviewer").unwrap();
        let stable = SubagentName::parse("stable").unwrap();
        let agent_directory = directory.join(".agents/agents");
        std::fs::create_dir_all(&agent_directory).unwrap();
        std::fs::write(
            agent_directory.join("reviewer.toml"),
            "instructions = 'Review changes'\n[model]\nmodel = 'local/a'\n",
        )
        .unwrap();
        std::fs::write(
            agent_directory.join("stable.toml"),
            "instructions = 'Review changes'\n[model]\nmodel = 'local/b'\n",
        )
        .unwrap();
        let settings = directory.join("rustx.toml");
        let before = std::fs::read_to_string(&settings).unwrap();
        std::fs::write(
            &settings,
            format!("{before}\n[agent]\nagents = ['reviewer', 'stable']\n"),
        )
        .unwrap();

        let create = || {
            fixture.manager.create_session(SessionPersistentState {
                session_config: None,
                session_resources: None,
                cwd: directory.clone(),
                model: None,
            })
        };
        let old_session = create().await.unwrap().session;
        let old = old_session.id;
        let old_binding = fixture
            .manager
            .sessions
            .configuration_bindings
            .lock()
            .unwrap()[&old]
            .clone();
        assert_eq!(
            old_binding.config.agent.agents,
            [agent.clone(), stable.clone()]
        );
        assert!(fixture.manager.configuration_runtime(&old).is_none());

        let original_model = issue459_authored_a(&fixture).await;
        let source = fixture
            .manager
            .source_settings(&SourceTarget::User)
            .await
            .unwrap();
        fixture
            .manager
            .write_source_settings(
                &SourceTarget::User,
                source.user.revision,
                SourceMutation::Config {
                    mutation: ConfigMutation::Model {
                        id: "local/a".into(),
                        authored: None,
                    },
                },
            )
            .await
            .unwrap();
        let source = fixture.manager.source_settings(&target).await.unwrap();
        let Some(SessionModelsView::Available {
            catalog,
            default_model,
        }) = source.session_models
        else {
            panic!("creation should use the updated catalog")
        };
        assert_eq!(default_model.model.as_str(), "local/b");
        assert_eq!(catalog.models.len(), 1);
        assert_eq!(catalog.models[0].model.as_str(), "local/b");

        let new_session = create().await.unwrap().session;
        let created = new_session.id;
        fixture.manager.load(&created, None).await.unwrap();
        let current = fixture.manager.configuration_runtime(&created).unwrap();
        assert_eq!(current.model_catalog(), catalog);
        assert_eq!(current.model_view().configured.model.as_str(), "local/b");
        let binding = fixture
            .manager
            .sessions
            .configuration_bindings
            .lock()
            .unwrap()[&created]
            .clone();
        assert_eq!(binding.config.agent.agents, std::slice::from_ref(&stable));
        assert!(binding.subagents.get(&agent).is_some());
        assert_eq!(old_binding.session_model().model.as_str(), "local/a");

        let mut changes = fixture.manager.configuration_changes();
        fixture.manager.load(&old, None).await.unwrap();
        let resumed = fixture.manager.configuration_runtime(&old).unwrap();
        while resumed.model_catalog() != catalog {
            changes.changed().await.unwrap();
        }
        let unavailable = resumed.model_view();
        assert_eq!(unavailable.configured.model.as_str(), "local/a");
        assert!(unavailable.unavailable.is_some());
        let (recovered, _) = fixture
            .manager
            .set_model(&old, *default_model)
            .await
            .unwrap();
        assert!(recovered.unavailable.is_none());
        assert_eq!(recovered.configured.model.as_str(), "local/b");

        fixture
            .manager
            .unload(&old_session.active_conversation_id)
            .await
            .unwrap();
        fixture
            .manager
            .unload(&new_session.active_conversation_id)
            .await
            .unwrap();
        assert!(fixture.manager.configuration_runtime(&old).is_none());
        assert!(fixture.manager.configuration_runtime(&created).is_none());
        write(
            &fixture,
            0,
            ConfigMutation::Model {
                id: "local/a".into(),
                authored: Some(original_model),
            },
        )
        .await;
        let restored = create().await.unwrap().session.id;
        let restored = fixture
            .manager
            .sessions
            .configuration_bindings
            .lock()
            .unwrap()[&restored]
            .clone();
        assert_eq!(restored.session_model().model.as_str(), "local/a");
        assert_eq!(
            restored.config.agent.agents,
            [agent.clone(), stable.clone()]
        );
        let main = restored.inspection.main.as_ref().unwrap();
        assert_eq!(main.agents, [agent, stable]);
        assert_ne!(
            main.agent_controls,
            [] as [crate::runtime::capability_inspection::ToolInspection; 0]
        );
        assert!(
            !restored
                .inspection
                .resource_diagnostics
                .iter()
                .any(|item| item.reason == "selected Agent model is unavailable")
        );
        fixture.close().await;
    }))
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn hidden_default_models_and_providers_recover_creation_without_reselecting_existing_sessions()
 {
    use crate::local_runtime::configuration::settings::{
        CredentialEdit, ProviderWrite, SessionModelsView, SourceTarget,
    };
    Box::pin(bounded(async {
        for hide_provider in [false, true] {
            let fixture = Fixture::with_session_count(None, 0).await;
            let directory = fixture.workspaces[0].clone();
            let target = SourceTarget::Workspace {
                directory: directory.clone(),
            };
            let path = fixture
                .manager
                .configuration
                .source_bindings()
                .config_path
                .clone();
            let mut document: toml::Value =
                toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            let other = document["providers"]["local"].clone();
            document["providers"]
                .as_table_mut()
                .unwrap()
                .insert("other".into(), other);
            document["models"]["local/b"]["provider"] = "other".into();
            std::fs::write(&path, toml::to_string(&document).unwrap()).unwrap();
            let create = || {
                fixture.manager.create_session(SessionPersistentState {
                    session_config: None,
                    session_resources: None,
                    cwd: directory.clone(),
                    model: None,
                })
            };
            let old = create().await.unwrap().session.id;
            fixture.manager.load(&old, None).await.unwrap();
            let runtime = fixture.manager.configuration_runtime(&old).unwrap();
            assert_eq!(runtime.model_view().configured.model.as_str(), "local/a");
            let authored = fixture
                .manager
                .source_settings(&SourceTarget::User)
                .await
                .unwrap()
                .user
                .authored
                .unwrap();
            let model_a = authored.models.as_ref().unwrap()["local/a"].clone();
            let model_b = authored.models.as_ref().unwrap()["local/b"].clone();
            let provider = authored.providers.as_ref().unwrap()["local"].clone();
            let toggle = |hidden| {
                if hide_provider {
                    ConfigMutation::Provider {
                        id: "local".into(),
                        authored: Some(ProviderWrite {
                            display_name: provider.display_name.clone(),
                            hidden,
                            icon: provider.icon.clone(),
                            protocol: provider.protocol,
                            base_url: provider.base_url.clone(),
                            credential: CredentialEdit::Retain,
                        }),
                    }
                } else {
                    let mut model = model_a.clone();
                    model.hidden = hidden;
                    ConfigMutation::Model {
                        id: "local/a".into(),
                        authored: Some(model),
                    }
                }
            };
            let mut changes = fixture.manager.configuration_changes();
            write(&fixture, 0, toggle(true)).await;
            let source = fixture.manager.source_settings(&target).await.unwrap();
            let Some(SessionModelsView::Available {
                catalog,
                default_model,
            }) = source.session_models
            else {
                panic!(
                    "a visible replacement must keep creation available: {:?}",
                    source.session_models
                );
            };
            assert_eq!(default_model.model.as_str(), "local/b");
            assert_eq!(
                catalog
                    .models
                    .iter()
                    .map(|model| model.model.as_str())
                    .collect::<Vec<_>>(),
                ["local/b"]
            );
            while runtime.model_catalog() != catalog {
                changes.changed().await.unwrap();
            }
            let retained = runtime.model_view();
            assert_eq!(retained.configured.model.as_str(), "local/a");
            assert!(retained.unavailable.is_none());
            assert_eq!(retained.effective.unwrap().model.as_str(), "local/a");
            let created = create().await.unwrap().session.id;
            fixture.manager.load(&created, None).await.unwrap();
            let created = fixture.manager.configuration_runtime(&created).unwrap();
            assert_eq!(created.model_view().configured.model.as_str(), "local/b");
            assert_eq!(created.model_catalog(), catalog);

            let mut hidden_b = model_b;
            hidden_b.hidden = true;
            write(
                &fixture,
                0,
                ConfigMutation::Model {
                    id: "local/b".into(),
                    authored: Some(hidden_b),
                },
            )
            .await;
            let empty = fixture.manager.source_settings(&target).await.unwrap();
            assert!(matches!(
                empty.session_models,
                Some(SessionModelsView::Unavailable { .. })
            ));
            assert!(
                create().await.is_err(),
                "all hidden is unavailable, never a hidden default"
            );

            write(&fixture, 0, toggle(false)).await;
            let restored = fixture.manager.source_settings(&target).await.unwrap();
            let Some(SessionModelsView::Available {
                catalog,
                default_model,
            }) = restored.session_models
            else {
                panic!("reenabling the authored default must restore creation");
            };
            assert_eq!(default_model.model.as_str(), "local/a");
            assert_eq!(catalog.models.len(), 1);
            let source: toml::Value =
                toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            assert_eq!(source["agent"]["model"]["model"].as_str(), Some("local/a"));
            let restored = create().await.unwrap().session.id;
            fixture.manager.load(&restored, None).await.unwrap();
            assert_eq!(
                fixture
                    .manager
                    .configuration_runtime(&restored)
                    .unwrap()
                    .model_view()
                    .configured
                    .model
                    .as_str(),
                "local/a"
            );
            fixture.close().await;
        }
    }))
    .await;
}

/// #419: creation is the only initial-model owner. Validate/persist before ACK,
/// without composing a runtime, then freeze that selection at first admission.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue419_initial_model_commit_readback_admission_and_later_switch() {
    Box::pin(bounded(async {
        for explicit in [false, true] {
            let f = Fixture::with_session_count(None, 0).await;
            let selection = crate::model::session::SessionModelConfig::of(
                crate::model::catalog::ModelRef::parse(if explicit {
                    "local/b"
                } else {
                    "local/a"
                })
                .unwrap(),
            );
            let created = f
                .manager
                .create_session(SessionPersistentState {
                    session_config: None,
                    session_resources: None,
                    cwd: f.workspaces[0].clone(),
                    model: explicit.then(|| selection.clone()),
                })
                .await
                .unwrap();
            assert!(created.durability_diagnostic.is_none());
            assert!(f.manager.registry.0.lock().unwrap().entries.is_empty());
            assert_eq!(f.provider.request_bodies(), [] as [std::string::String; 0]);
            let persisted =
                std::fs::read_to_string(f.archive_root.join("sessions/catalog.json")).unwrap();
            assert!(persisted.contains(&selection.model.to_string()));
            assert_eq!(
                f.manager
                    .sessions
                    .read_settings(&created.session.id)
                    .await
                    .unwrap()
                    .1
                    .model,
                Some(selection.clone())
            );
            let managed = f.manager.load(&created.session.id, None).await.unwrap();
            let runtime = f
                .manager
                .configuration_runtime(&created.session.id)
                .unwrap();
            assert_eq!(runtime.model_view().configured, selection);
            runtime.submit_inbound(input("request-A")).unwrap();
            f.gates[0].wait_entered().await;
            let before = managed.client().checkpoint().unwrap().snapshot;
            assert_eq!(
                before
                    .attempt
                    .as_ref()
                    .unwrap()
                    .model
                    .as_ref()
                    .unwrap()
                    .primary
                    .model,
                selection.model
            );
            let later = crate::model::session::SessionModelConfig::of(
                crate::model::catalog::ModelRef::parse(if explicit {
                    "local/a"
                } else {
                    "local/b"
                })
                .unwrap(),
            );
            assert!(
                f.manager
                    .set_model(&created.session.id, later.clone())
                    .await
                    .is_ok(),
                "live selection commits while the running Attempt retains its snapshot"
            );
            let after = managed.client().checkpoint().unwrap().snapshot;
            assert_eq!(before.attempt.unwrap().model, after.attempt.unwrap().model);
            let mut settlement = SettlementReceiptWait::capture(&runtime);
            f.gates[0].release();
            settlement.wait().await;
            assert_eq!(runtime.model_view().configured, later);
            f.manager
                .unload(&created.session.active_conversation_id)
                .await
                .unwrap();
        }
    }))
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue419_omitted_intent_uses_native_default_at_creation_not_draft_projection() {
    use crate::local_runtime::configuration::settings::{SessionModelsView, SourceTarget};
    Box::pin(bounded(async {
        let f = Fixture::with_session_count(None, 0).await;
        let target = SourceTarget::Workspace {
            directory: f.workspaces[0].clone(),
        };
        let projected = f.manager.source_settings(&target).await.unwrap();
        let Some(SessionModelsView::Available { default_model, .. }) = projected.session_models
        else {
            panic!()
        };
        assert_eq!(default_model.model.to_string(), "local/a");
        let path = f
            .manager
            .source_settings(&SourceTarget::User)
            .await
            .unwrap()
            .user
            .path;
        let mut document: toml::Value =
            toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        document["agent"]["model"]["model"] = "local/b".into();
        std::fs::write(&path, toml::to_string(&document).unwrap()).unwrap();
        f.manager.reconcile_configuration(&target).await.unwrap();
        source_settled(&f, &target).await;
        let created = f
            .manager
            .create_session(SessionPersistentState {
                session_config: None,
                session_resources: None,
                cwd: f.workspaces[0].clone(),
                model: None,
            })
            .await
            .unwrap();
        assert_eq!(
            f.manager
                .sessions
                .read_settings(&created.session.id)
                .await
                .unwrap()
                .1
                .model
                .unwrap()
                .model
                .to_string(),
            "local/b"
        );
        let runtime = f.manager.load(&created.session.id, None).await.unwrap();
        let native = f
            .manager
            .configuration_runtime(&created.session.id)
            .unwrap();
        native.submit_inbound(input("request-A")).unwrap();
        f.gates[0].wait_entered().await;
        assert_eq!(
            runtime
                .client()
                .checkpoint()
                .unwrap()
                .snapshot
                .attempt
                .unwrap()
                .model
                .unwrap()
                .primary
                .model
                .to_string(),
            "local/b"
        );
        let mut settlement = SettlementReceiptWait::capture(&native);
        f.gates[0].release();
        settlement.wait().await;
        f.manager
            .unload(&created.session.active_conversation_id)
            .await
            .unwrap();
    }))
    .await;
}

#[tokio::test]
async fn issue419_rejected_explicit_model_does_not_publish_or_substitute() {
    let f = Fixture::with_session_count(None, 0).await;
    let result = f
        .manager
        .create_session(SessionPersistentState {
            session_config: None,
            session_resources: None,
            cwd: f.workspaces[0].clone(),
            model: Some(crate::model::session::SessionModelConfig::of(
                crate::model::catalog::ModelRef::parse("local/missing").unwrap(),
            )),
        })
        .await;
    assert!(result.is_err());
    assert_eq!(
        f.manager
            .sessions
            .list_sessions(None, 0, 32)
            .await
            .unwrap()
            .sessions,
        [] as [crate::local_runtime::session::SessionSummary; 0]
    );
    assert!(f.manager.registry.0.lock().unwrap().entries.is_empty());
    assert_eq!(f.provider.request_bodies(), [] as [std::string::String; 0]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue422_creation_binding_precedes_multi_client_visibility() {
    use crate::app_server::{connection::AppServerConnection, protocol::*};
    use crate::local_runtime::configuration::settings::SourceTarget;
    Box::pin(bounded(async {
        for (uncertain, submitted) in [(false, false), (true, false), (false, true), (true, true)] {
            let f = Fixture::with_session_count(None, 1).await;
            f.manager.load(&f.sessions[0].id, None).await.unwrap();
            f.manager
                .source_settings(&SourceTarget::Workspace {
                    directory: f.workspaces[0].clone(),
                })
                .await
                .unwrap();
            let creator = AppServerConnection::new(f.host.clone());
            let observer = AppServerConnection::new(f.host.clone());
            super::super::protocol::initialize(&creator).await;
            super::super::protocol::initialize(&observer).await;
            let gate = Arc::new(crate::runtime::conversation_runtime::Gate::default());
            let release = gate.arm_scoped();
            *f.manager.sessions.create_gate.lock().unwrap() = Some(gate.clone());
            let (visible, visibility) = tokio::sync::oneshot::channel();
            let (finish, finishing) = std::sync::mpsc::channel();
            f.manager
                .configuration
                .test_hooks
                .insert("create_after_visibility", move || {
                    visible.send(()).unwrap();
                    finishing.recv().unwrap();
                });
            let cwd = f.workspaces[0].clone();
            let create = tokio::spawn(async move {
                super::super::protocol::call(
                    &creator,
                    1,
                    Method::SessionCreate {
                        input_references: None,
                        dynamic_tools: Vec::new(),
                        submission: submitted.then(|| {
                            crate::local_runtime::mcp_drafts::McpDraftTarget {
                                draft_id: "visibility-regression".into(),
                                generation: 1,
                                host_authority: "fixture-host".into(),
                            }
                        }),
                        settings: SessionPersistentState {
                            cwd,
                            model: None,
                            session_config: None,
                            session_resources: None,
                        },
                    },
                )
                .await
            });
            tokio::task::spawn_blocking(move || gate.wait_entered())
                .await
                .unwrap();
            // The creation owner already captured A; private preparation is gated.
            assert_eq!(
                f.manager
                    .sessions
                    .configuration_bindings
                    .lock()
                    .unwrap()
                    .len(),
                1
            );
            assert_eq!(
                f.manager
                    .sessions
                    .list_sessions(None, 0, 32)
                    .await
                    .unwrap()
                    .sessions
                    .len(),
                1
            );
            let path = f
                .manager
                .source_settings(&SourceTarget::User)
                .await
                .unwrap()
                .user
                .path;
            let mut document: toml::Value =
                toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            let instructions_a = f
                .manager
                .source_settings(&SourceTarget::User)
                .await
                .unwrap()
                .global_guidance
                .unwrap()
                .authored
                .unwrap_or_default();
            document["agent"]["model"]["model"] = "local/b".into();
            set_file_guidance(&f, "configuration B");
            std::fs::write(&path, toml::to_string(&document).unwrap()).unwrap();
            let source = SourceTarget::Workspace {
                directory: f.workspaces[0].clone(),
            };
            f.manager.reconcile_configuration(&source).await.unwrap();
            source_settled(&f, &source).await;
            settled(&f, 0).await;
            // A fresh creation capture really would select B, not merely a changed file.
            let fresh = f
                .manager
                .applications
                .lock()
                .initial_binding(
                    &f.manager.configuration,
                    &SessionConfigInput {
                        authoring:
                            crate::local_runtime::session_inputs::SessionAuthoredInputs::default(),
                        cwd: f.workspaces[0].clone(),
                        model: None,
                    },
                    &f.manager.credentials,
                    false,
                )
                .unwrap();
            assert_eq!(fresh.session_model().model.to_string(), "local/b");
            assert_eq!(guidance(&fresh.project_context_files), "configuration B");
            if uncertain {
                f.manager
                    .sessions
                    .catalog
                    .lock()
                    .await
                    .arm_write_fault_after_rename();
            }
            drop(release);
            visibility.await.unwrap(); // Creator is parked before scope bookkeeping/ACK.
            let id = loop {
                if let NotificationMethod::SummaryInvalidated {
                    session_id,
                    catalog_changed: true,
                } = observer.next_notification().await.notification
                {
                    break session_id;
                }
            };
            let MethodResult::Sessions { sessions, .. } = super::super::protocol::call(
                &observer,
                2,
                Method::SessionList {
                    query: None,
                    offset: 0,
                    limit: 32,
                },
            )
            .await
            else {
                panic!()
            };
            assert_eq!(sessions.len(), 2);
            assert!(sessions.iter().any(|session| session.id == id));
            let retained = f.manager.sessions.configuration_bindings.lock().unwrap()[&id].clone();
            assert_eq!(retained.session_model().model.to_string(), "local/a");
            assert_ne!(guidance(&retained.project_context_files), "configuration B");
            assert_eq!(guidance(&retained.project_context_files), instructions_a);
            let MethodResult::Attached { target, .. } = super::super::protocol::call(
                &observer,
                3,
                Method::SessionAttach {
                    thread_id: f
                        .manager
                        .sessions
                        .read_session(&id)
                        .await
                        .unwrap()
                        .active_conversation_id,
                },
            )
            .await
            else {
                panic!()
            };
            let runtime = f.manager.configuration_runtime(&id).unwrap();
            assert_eq!(
                guidance(runtime.runtime_resources().project_context_files()),
                guidance(&retained.project_context_files)
            );
            assert_eq!(runtime.model_view().configured, *retained.session_model());
            assert!(!create.is_finished());
            // The ACK registers this Session with desired source B. A later
            // warm load can publish its catalog without changing selected A.
            // Keep that ordinary publication separate from creation ownership.
            let publication = f.manager.probe(&target.conversation_id);
            publication.before_catalog_publication.arm();
            finish.send(()).unwrap();
            let MethodResult::SessionTransition {
                session,
                durability_diagnostic,
                ..
            } = create.await.unwrap()
            else {
                panic!()
            };
            assert_eq!(session.id, id);
            assert_eq!(
                f.manager
                    .probe(&session.active_conversation_id)
                    .compositions
                    .load(Ordering::SeqCst),
                1
            );
            assert_eq!(
                f.manager.load(&id, None).await.unwrap().incarnation_id(),
                target.runtime_incarnation
            );
            assert_eq!(durability_diagnostic.is_some(), uncertain);
            publication.before_catalog_publication.entered().await;
            assert_eq!(
                f.manager.sessions.configuration_bindings.lock().unwrap()[&id].binding_revision,
                retained.binding_revision
            );
            assert_eq!(runtime.model_view().configured, *retained.session_model());
            assert_eq!(
                guidance(runtime.runtime_resources().project_context_files()),
                guidance(&retained.project_context_files)
            );
            publication.before_catalog_publication.release();
            // The catalog publishes, then the desired instructions commit
            // automatically; the Session's own selection is never replaced.
            let application = session_applied(&f, &id).await;
            assert_eq!(
                application.units[&ApplyUnit::Instructions],
                UnitApplication::Applied
            );
            let published = f.manager.sessions.configuration_bindings.lock().unwrap()[&id].clone();
            assert!(published.binding_revision > retained.binding_revision);
            assert_eq!(
                runtime.configuration_cut().binding_revision,
                published.binding_revision
            );
            assert_eq!(runtime.model_view().configured, *retained.session_model());
            assert_eq!(
                guidance(runtime.runtime_resources().project_context_files()),
                "configuration B"
            );
            assert_eq!(
                f.manager.load(&id, None).await.unwrap().incarnation_id(),
                target.runtime_incarnation
            );
            super::super::protocol::call(&observer, 4, Method::SessionDetach { target }).await;
            f.close().await;
        }
    }))
    .await;
}

#[tokio::test]
async fn issue422_creation_previsibility_failure_removes_provisional_binding() {
    let f = Fixture::with_session_count(None, 0).await;
    let log = f.manager.sessions.summary_invalidations();
    let frontier = log.frontier();
    f.manager
        .sessions
        .catalog
        .lock()
        .await
        .arm_write_fault_before_rename();
    assert!(
        f.manager
            .create_session(SessionPersistentState {
                session_config: None,
                session_resources: None,
                cwd: f.workspaces[0].clone(),
                model: None
            })
            .await
            .is_err()
    );
    assert!(
        f.manager
            .sessions
            .configuration_bindings
            .lock()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        f.manager
            .sessions
            .list_sessions(None, 0, 32)
            .await
            .unwrap()
            .sessions,
        [] as [crate::local_runtime::session::SessionSummary; 0]
    );
    assert_eq!(log.next_after(frontier), None);
    f.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn disabled_workflow_missing_agent_does_not_block_session_creation() {
    let fixture = Fixture::with_session_count(None, 0).await;
    let directory = fixture.workspaces[0].clone();
    let workflow = directory.join(".agents/workflows/review.yaml");
    std::fs::create_dir_all(workflow.parent().unwrap()).unwrap();
    let schema = serde_json::json!({"type":"object","properties":{},"additionalProperties":false});
    std::fs::write(&workflow, serde_json::to_string(&serde_json::json!({
        "description":"Unavailable reviewer", "block":{"input":schema,"nodes":{
            "start":{"type":"start"},
            "review":{"type":"agent","profile":"missing","task":[{"type":"literal","value":"Review"}],"output":{"type":"json_schema","schema":schema}},
            "done":{"type":"return","outputs":[]}},"edges":[{"from":"start","to":"review"},{"from":"review","to":"done"}]}
    })).unwrap()).unwrap();
    std::fs::write(
        directory.join("rustx.toml"),
        "[agent]\nworkflows=['review']\n",
    )
    .unwrap();
    let session = fixture
        .manager
        .create_session(SessionPersistentState {
            session_config: None,
            session_resources: None,
            cwd: directory.clone(),
            model: None,
        })
        .await
        .unwrap();
    let loaded = fixture
        .manager
        .load(&session.session.id, None)
        .await
        .unwrap();
    let resources = loaded.inspect_runtime().unwrap().runtime_resources();
    assert!(resources.workflows().enabled_ids().is_empty());
    let id = crate::runtime::workflow::WorkflowId::parse("review").unwrap();
    assert!(matches!(&resources.workflows().entries()[&id].admission,
        crate::runtime::workflow::WorkflowAdmission::Disabled(diagnostics)
        if diagnostics.iter().any(|diagnostic| matches!(&diagnostic.reason,
            crate::runtime::workflow::WorkflowDependencyFailure::Agent(
                crate::runtime::agent_profile::AgentProfileDiagnostic::AgentUnavailable { .. })) )));
    assert_eq!(
        std::fs::read_to_string(directory.join("rustx.toml")).unwrap(),
        "[agent]\nworkflows=['review']\n"
    );
    fixture.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn disabled_workflow_external_sources_do_not_block_session_creation_or_reload() {
    for case in ["missing", "failed_mcp", "failed_python", "missing_agent"] {
        let fixture = Fixture::with_session_count(None, 0).await;
        let directory = fixture.workspaces[0].clone();
        let agents = directory.join(".agents");
        std::fs::create_dir_all(agents.join("workflows")).unwrap();
        let source = if case == "failed_python" {
            let package = agents.join("tools/broken");
            std::fs::create_dir_all(&package).unwrap();
            // A demanded package is rejected before runtime/network preparation.
            std::fs::write(
                package.join("server.py"),
                "raise RuntimeError('unusable')\n",
            )
            .unwrap();
            "python:broken"
        } else {
            "absent"
        };
        if case == "failed_mcp" {
            std::fs::write(
                agents.join("mcp.toml"),
                format!(
                    "[mcp_servers.absent]\ntype = 'stdio'\ncommand = {:?}\n",
                    directory.join("missing-mcp-executable").to_str().unwrap()
                ),
            )
            .unwrap();
        }
        let schema =
            serde_json::json!({"type":"object","properties":{},"additionalProperties":false});
        let mut nodes = serde_json::json!({"start":{"type":"start"},
            "external":{"type":"tool","selector":{"origin":"source","source_id":source,"name":"inspect"},"arguments":{"type":"literal","value":{}},"result":{"type":"text"}},
            "done":{"type":"return","outputs":[]}});
        let mut edges =
            serde_json::json!([{"from":"start","to":"external"},{"from":"external","to":"done"}]);
        if case == "missing_agent" {
            nodes["review"] = serde_json::json!({"type":"agent","profile":"missing","task":[{"type":"literal","value":"Review"}],"output":{"type":"text"}});
            edges = serde_json::json!([{"from":"start","to":"external"},{"from":"external","to":"review"},{"from":"review","to":"done"}]);
        }
        std::fs::write(agents.join("workflows/inspect.yaml"), serde_json::to_string(&serde_json::json!({
            "description":"Optional external inspection", "block":{"input":schema,"nodes":nodes,"edges":edges}
        })).unwrap()).unwrap();
        let configuration = "[agent]\nworkflows=['inspect']\n";
        std::fs::write(directory.join("rustx.toml"), configuration).unwrap();
        let created = fixture
            .manager
            .create_session(SessionPersistentState {
                session_config: None,
                session_resources: None,
                cwd: directory.clone(),
                model: None,
            })
            .await
            .unwrap();
        for reload in [false, true] {
            if reload {
                fixture
                    .manager
                    .unload(&created.session.active_conversation_id)
                    .await
                    .unwrap();
            }
            let loaded = fixture
                .manager
                .load(&created.session.id, None)
                .await
                .unwrap();
            let resources = loaded.inspect_runtime().unwrap().runtime_resources();
            assert!(
                resources.workflows().enabled_ids().is_empty(),
                "{case}, reload={reload}"
            );
            let id = crate::runtime::workflow::WorkflowId::parse("inspect").unwrap();
            assert!(
                matches!(&resources.workflows().entries()[&id].admission,
                crate::runtime::workflow::WorkflowAdmission::Disabled(diagnostics) if !diagnostics.is_empty()),
                "{case}, reload={reload}"
            );
            assert_eq!(
                std::fs::read_to_string(directory.join("rustx.toml")).unwrap(),
                configuration
            );
        }
        fixture.close().await;
    }
}

#[cfg(target_os = "linux")]
fn workspace_overlay_launcher(mount: &std::path::Path) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt as _;
    let launcher = mount.join("client");
    let staged = std::env::current_exe()
        .unwrap()
        .with_file_name("rustx-execution-client");
    std::os::unix::fs::symlink(staged, mount.join("client-real")).unwrap();
    std::fs::write(
        &launcher,
        r#"#!/bin/sh
set -eu
bin=${0%/*}
unset RUSTX_CLIENT_DOMAIN_STAGING
exec /usr/bin/bwrap --die-with-parent --ro-bind / / --dev /dev --tmpfs "${4%/*}" --bind "$bin/physical" "$4" -- "$bin/client-real" "$@"
"#,
    ).unwrap();
    std::fs::set_permissions(&launcher, std::fs::Permissions::from_mode(0o700)).unwrap();
    launcher
}

#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "opt-in bwrap proof; run test-support/prove_execution_mount_view.sh"]
async fn bound_configuration_load_and_reconcile_without_any_server_workspace_directory() {
    use super::super::protocol::{call, initialize};
    use crate::app_server::{
        connection::AppServerConnection,
        protocol::{ErrorData, Method, MethodResult},
    };
    use crate::local_runtime::configuration::settings::SourceTarget;
    use crate::runtime::subagent::SubagentName;
    use crate::runtime::workflow::WorkflowId;
    use base64::Engine as _;
    assert!(std::env::var_os("RUSTX_MOUNT_VIEW_PROOF").is_some());
    Box::pin(bounded(async {
        let fixture = Fixture::with_session_count(None, 2).await;
        let workspace = &fixture.workspaces[0];
        let mount = tempfile::tempdir().unwrap();
        let physical = mount.path().join("physical");
        std::fs::rename(workspace, &physical).unwrap();
        let document = physical.join("rustx.toml");
        let write_config = |deadline| {
            std::fs::write(&document, format!(
                "[agent]\nagents=['reviewer']\nworkflows=['client-resource']\n[tool_deadline_policy]\nhard_deadline_ms={deadline}\n"
            )).unwrap();
        };
        write_config(23_456);
        let agent_dir = physical.join(".agents/agents");
        let workflow_dir = physical.join(".agents/workflows");
        std::fs::create_dir_all(&agent_dir).unwrap();
        std::fs::create_dir_all(&workflow_dir).unwrap();
        let role = agent_dir.join("reviewer.toml");
        let workflow = workflow_dir.join("client-resource.yaml");
        let write_agent = |instructions| {
            std::fs::write(&role, format!("description='Review'\ninstructions='{instructions}'\nskills=[]\n")).unwrap();
        };
        let write_workflow = |description| {
            std::fs::write(&workflow, format!(
                "description: {description}\nblock:\n  input: {{type: object, properties: {{}}, additionalProperties: false}}\n  edges: [{{from: workflow_start, to: done}}]\n  nodes:\n    workflow_start: {{type: start}}\n    done: {{type: return, outputs: []}}\n"
            )).unwrap();
        };
        write_agent("Client-only Agent");
        write_workflow("Client-only Workflow");
        let instructions = physical.join("AGENTS.md");
        std::fs::write(&instructions, "\u{feff}Client instructions").unwrap();
        let package = physical.join(".agents/skills/bound-skill");
        std::fs::create_dir_all(&package).unwrap();
        let skill_document = b"---\nname: bound-skill\ndescription: Client-only Skill\n---\nClient-only body\n";
        std::fs::write(package.join("SKILL.md"), skill_document).unwrap();
        let launcher = workspace_overlay_launcher(mount.path());
        assert!(!workspace.exists());
        let mut client = fixture.start_execution_via(0, Some(&launcher)).await;
        fixture.load(0).await.unwrap().unwrap();
        let retained = || {
            fixture.manager.sessions.configuration_bindings.lock().unwrap()[&fixture.sessions[0].id]
                .config.tool_deadline_policy.hard_deadline_ms
        };
        assert_eq!(retained(), 23_456);
        let runtime = fixture.manager.configuration_runtime(&fixture.sessions[0].id).unwrap();
        let assert_instructions = |expected: &str| {
            let resources = runtime.runtime_resources();
            let file = resources.project_context_files().iter()
                .find(|file| file.path == workspace.join("AGENTS.md")).unwrap();
            assert_eq!(file.content, expected);
        };
        assert_instructions("Client instructions");
        let reviewer = SubagentName::parse("reviewer").unwrap();
        let workflow_id = WorkflowId::parse("client-resource").unwrap();
        let assert_definitions = |agent_text: &str, workflow_description: &str| {
            let resources = runtime.runtime_resources();
            let agent = resources.subagents().get(&reviewer).unwrap();
            assert_eq!(agent.instructions(), agent_text);
            let workflows = resources.workflows();
            assert_eq!(workflows.enabled_ids(), std::collections::BTreeSet::from([workflow_id.clone()]));
            assert_eq!(workflows.get(&workflow_id).unwrap().description(), workflow_description);
        };
        assert_definitions("Client-only Agent", "Client-only Workflow");

        let source = SourceTarget::Workspace { directory: workspace.clone() };
        let connection = AppServerConnection::new(fixture.host.clone());
        initialize(&connection).await;
        let request = |index: usize| Method::SkillsList {
            target: source.clone(),
            execution_session: Some(fixture.sessions[index].id.clone()),
            offset: 0,
            limit: 32,
            expected_revision: None,
        };
        let MethodResult::Skills { page, authority } = call(&connection, 50, request(0)).await else { panic!("live Workspace Skills"); };
        assert_eq!(authority, crate::app_server::protocol::WorkspaceCatalogAuthority::Client);
        let [descriptor] = page.data.as_slice() else { panic!("one Skill: {page:?}"); };
        assert_eq!(descriptor.catalog.name, "bound-skill");
        assert_eq!(descriptor.catalog.location, workspace.join(".agents/skills/bound-skill/SKILL.md").display().to_string());
        assert!(descriptor.operation_revision.is_none());
        let read = |offset, expected_revision| Method::SkillsRead {
            target: source.clone(),
            execution_session: Some(fixture.sessions[0].id.clone()),
            name: "bound-skill".into(),
            kind: crate::local_runtime::skill_management::SkillContentKind::Instructions,
            offset,
            max_bytes: 51,
            expected_revision,
        };
        let MethodResult::SkillContent { content: first } = call(&connection, 51, read(0, None)).await else { panic!("first bounded page"); };
        let MethodResult::SkillContent { content: rest } = call(
            &connection, 52, read(first.next_offset.unwrap(), Some(first.revision.clone())),
        ).await else { panic!("last bounded page"); };
        assert_eq!(rest.revision, first.revision);
        assert!(rest.next_offset.is_none());
        let decoded: Vec<_> = [first.data_base64, rest.data_base64].into_iter()
            .flat_map(|page| base64::engine::general_purpose::STANDARD.decode(page).unwrap())
            .collect();
        assert_eq!(decoded, skill_document);
        {
            use crate::app_server::protocol::{JsonRpcVersion, Request, RequestId, Response};
            let response = connection.handle_request(Request {
                jsonrpc: JsonRpcVersion::V2,
                id: RequestId::Integer(53),
                call: request(1),
            }).await;
            let Response::Failure(failure) = response else { panic!("an unbound Session cannot list the first Session's Skills"); };
            assert_eq!(failure.error.data, Some(ErrorData::ExecutionUnavailable));
        }
        let old = {
            let state = fixture.manager.applications.lock();
            state.view(fixture.sessions[0].id.as_ref()).and_then(|application| application.desired.input_revision)
        };
        write_config(34_567);
        write_agent("Reconciled Agent");
        write_workflow("Reconciled Workflow");
        std::fs::write(&instructions, "Reconciled instructions").unwrap();
        let observation = fixture.manager.reconcile_configuration(&source).await.unwrap();
        assert_eq!(observation.scope, source.application_scope());
        assert!(observation.desired.input_revision.is_some());
        let application = applied(&fixture, 0).await;
        assert_ne!(application.desired.input_revision, old);
        assert_eq!(application.units[&ApplyUnit::ExecutionPolicy], UnitApplication::Applied);
        assert_eq!(application.units[&ApplyUnit::Instructions], UnitApplication::Applied);
        assert_instructions("Reconciled instructions");
        assert_definitions("Reconciled Agent", "Reconciled Workflow");
        let effective = crate::local_runtime::configuration::settings::EffectiveConfiguration::adopted(&runtime).unwrap();
        assert_eq!(effective.tool_deadline.hard_deadline_ms, 34_567);

        // A Server directory at the same spelling is a decoy. Bound
        // reconciliation must not run the Server's authoring repair over it.
        std::fs::create_dir(workspace).unwrap();
        let decoy = workspace.join("rustx.toml");
        let decoy_text = "[agent.plugins.todo]\nenabled = true\nlegacy = 'do not repair'\n";
        std::fs::write(&decoy, decoy_text).unwrap();
        std::fs::write(&instructions, [0xff]).unwrap();
        fixture.manager.reconcile_configuration(&source).await.unwrap();
        assert_eq!(std::fs::read_to_string(&decoy).unwrap(), decoy_text);
        std::fs::remove_file(decoy).unwrap();
        std::fs::remove_dir(workspace).unwrap();
        let application = settled(&fixture, 0).await;
        assert!(matches!(&application.units[&ApplyUnit::Instructions],
            UnitApplication::Failed { diagnostic } if diagnostic.contains("instruction text is not UTF-8")
        ), "{application:?}");
        assert_instructions("Reconciled instructions");
        std::fs::write(&instructions, "Reconciled instructions").unwrap();

        // The retained Client binding must also remain admissible when the
        // same spelling names a non-directory in the Server's mount.
        std::fs::write(workspace, b"unrelated Server file").unwrap();
        write_config(45_678);
        fixture.manager.reconcile_configuration(&source).await.unwrap();
        let resolution = applied(&fixture, 0).await;
        assert_eq!(resolution.units[&ApplyUnit::ExecutionPolicy], UnitApplication::Applied);
        assert_eq!(retained(), 45_678);
        assert_eq!(std::fs::read(workspace).unwrap(), b"unrelated Server file");
        std::fs::remove_file(workspace).unwrap();

        let changed_during_capture = workflow.clone();
        let stable_workflow = std::fs::read(&workflow).unwrap();
        fixture.manager.configuration.test_hooks.insert("before_resource_verification", move || {
            std::fs::write(changed_during_capture, b"content changed after the definition read").unwrap();
        });
        fixture.manager.reconcile_configuration(&source).await.unwrap();
        let application = settled(&fixture, 0).await;
        assert!(matches!(
            &application.units[&ApplyUnit::Capabilities],
            UnitApplication::Failed { diagnostic } if diagnostic.contains("resource definitions changed during capture")
        ), "{application:?}");
        assert_definitions("Reconciled Agent", "Reconciled Workflow");
        std::fs::write(&workflow, stable_workflow).unwrap();

        std::fs::write(&role, "invalid = [").unwrap();
        fixture.manager.reconcile_configuration(&source).await.unwrap();
        let application = settled(&fixture, 0).await;
        assert!(matches!(
            &application.units[&ApplyUnit::Capabilities],
            UnitApplication::Failed { diagnostic } if diagnostic == "canonical Agent TOML violates its strict typed authoring contract"
        ), "{application:?}");
        assert_definitions("Reconciled Agent", "Reconciled Workflow");
        assert!(!workspace.exists());
        assert_eq!(fixture.host.drain().await, [] as [String; 0]);
        assert!(client.wait().await.unwrap().success());
    }))
    .await;
}

#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "opt-in bwrap proof; run test-support/prove_execution_mount_view.sh"]
async fn bound_reconcile_keeps_the_admitted_agent_when_its_workspace_source_is_lost() {
    use crate::local_runtime::configuration::settings::{EffectiveConfiguration, SourceTarget};
    use crate::runtime::subagent::SubagentName;
    assert!(std::env::var_os("RUSTX_MOUNT_VIEW_PROOF").is_some());
    Box::pin(bounded(async {
        let fixture = Fixture::with_session_count(None, 1).await;
        let workspace = &fixture.workspaces[0];
        let mount = tempfile::tempdir().unwrap();
        let physical = mount.path().join("physical");
        std::fs::create_dir(&physical).unwrap();
        let document = physical.join("rustx.toml");
        let write_policy = |deadline| {
            std::fs::write(&document, format!(
                "[agent]\nagents=['reviewer']\nskills=[]\n[tool_deadline_policy]\nhard_deadline_ms={deadline}\n"
            )).unwrap();
        };
        write_policy(23_456);
        let user_agents = fixture.manager.configuration.source_bindings().home_directory.join("rustx/.agents/agents");
        let workspace_agents = physical.join(".agents/agents");
        let server_decoy = workspace.join(".agents/agents");
        let write_role = |root: &std::path::Path, label| {
            std::fs::create_dir_all(root).unwrap();
            std::fs::write(root.join("reviewer.toml"), format!("description='Review'\ninstructions='{label}'\nskills=[]\n")).unwrap();
        };
        write_role(&user_agents, "User reviewer");
        write_role(&workspace_agents, "Workspace reviewer");
        write_role(&server_decoy, "Server decoy reviewer");
        let launcher = workspace_overlay_launcher(mount.path());
        let mut client = fixture.start_execution_via(0, Some(&launcher)).await;
        fixture.load(0).await.unwrap().unwrap();
        let runtime = fixture.manager.configuration_runtime(&fixture.sessions[0].id).unwrap();
        let reviewer = SubagentName::parse("reviewer").unwrap();
        let instructions = || runtime.runtime_resources().subagents().get(&reviewer).unwrap().instructions().to_owned();
        assert_eq!(instructions(), "Workspace reviewer");

        let original_root = workspace.with_extension("retained");
        std::fs::rename(workspace, &original_root).unwrap();
        let target = SourceTarget::Workspace { directory: workspace.clone() };
        for (deadline, replacement, label) in [
            (34_567, false, "Client reviewer with no Server Workspace"),
            (45_678, true, "Client reviewer with a replacement Server Workspace"),
        ] {
            if replacement {
                std::fs::create_dir(workspace).unwrap();
            }
            write_role(&workspace_agents, label);
            write_policy(deadline);
            fixture.manager.reconcile_configuration(&target).await.unwrap();
            let application = applied(&fixture, 0).await;
            assert_eq!(application.units[&ApplyUnit::ExecutionPolicy], UnitApplication::Applied);
            assert_eq!(application.units[&ApplyUnit::Capabilities], UnitApplication::Applied);
            assert_eq!(EffectiveConfiguration::adopted(&runtime).unwrap().tool_deadline.hard_deadline_ms, deadline);
            assert_eq!(instructions(), label);
        }

        std::fs::remove_dir(workspace).unwrap();
        std::fs::rename(original_root, workspace).unwrap();
        let invalid_collection = physical.join("unavailable-agents");
        std::fs::write(&invalid_collection, b"not a collection").unwrap();
        // A confirmed absent layer can reveal lower definitions. Swap the
        // physical collection atomically so the watcher observes an invalid root.
        let exchange_collection = || {
            rustix::fs::renameat_with(
                rustix::fs::CWD, &workspace_agents, rustix::fs::CWD,
                &invalid_collection, rustix::fs::RenameFlags::EXCHANGE,
            ).unwrap();
        };
        exchange_collection();
        write_policy(56_789);
        fixture.manager.reconcile_configuration(&target).await.unwrap();
        let application = applied(&fixture, 0).await;
        assert_eq!(application.units[&ApplyUnit::ExecutionPolicy], UnitApplication::Applied);
        assert!(matches!(&application.units[&ApplyUnit::Capabilities],
            UnitApplication::Failed { diagnostic } if diagnostic.contains("AgentUnavailable") && diagnostic.contains("reviewer")
        ), "{application:?}");
        assert_eq!(EffectiveConfiguration::adopted(&runtime).unwrap().tool_deadline.hard_deadline_ms, 56_789);
        assert_eq!(instructions(), "Client reviewer with a replacement Server Workspace");

        exchange_collection();
        write_role(&workspace_agents, "Restored Workspace reviewer");
        fixture.manager.reconcile_configuration(&target).await.unwrap();
        let application = applied(&fixture, 0).await;
        assert_eq!(application.units[&ApplyUnit::Capabilities], UnitApplication::Applied);
        assert_eq!(instructions(), "Restored Workspace reviewer");
        assert_eq!(fixture.host.drain().await, [] as [String; 0]);
        assert!(client.wait().await.unwrap().success());
    })).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn session_inputs_commit_together_and_require_permission_for_a_replaced_workflow() {
    use super::super::protocol::{call, initialize};
    use crate::app_server::connection::AppServerConnection;
    use crate::app_server::protocol::{ErrorData, Method, MethodResult};
    use crate::local_runtime::session_inputs::{
        SessionConfigContent, SessionInputChange, SessionInputReference, SessionInputReferences,
        SessionResources,
    };
    use crate::runtime::subagent::SubagentName;
    use crate::runtime::workflow::WorkflowId;
    use std::collections::BTreeMap;

    Box::pin(bounded(async {
        let fixture = Fixture::with_session_count(None, 1).await;
        let id = fixture.sessions[0].id.clone();
        let workspace = &fixture.workspaces[0];
        let review = WorkflowId::parse("review").unwrap();
        let helper = SubagentName::parse("reviewer").unwrap();
        let workflow_document = |description| format!(
            "description: {description}\nblock:\n  input: {{type: object, properties: {{}}, additionalProperties: false}}\n  edges: [{{from: workflow_start, to: done}}]\n  nodes:\n    workflow_start: {{type: start}}\n    done: {{type: return, outputs: []}}\n"
        );
        let workflows = workspace.join(".agents/workflows");
        std::fs::create_dir_all(&workflows).unwrap();
        std::fs::write(workflows.join("review.yaml"), workflow_document("Workspace review")).unwrap();
        std::fs::write(workspace.join("rustx.toml"), "[agent]\nworkflows=['review']\n").unwrap();

        let connection = AppServerConnection::new(fixture.host.clone());
        initialize(&connection).await;
        let mut client = fixture.start_execution(0).await;
        fixture.load(0).await.unwrap().unwrap();
        let runtime = fixture.manager.configuration_runtime(&id).unwrap();
        let catalog = || runtime.runtime_resources();
        let ready = || catalog().workflows().enabled_ids().contains(&review);
        let description = || catalog().workflows().get(&review).unwrap().description().to_owned();
        assert!(ready());
        assert_eq!(description(), "Workspace review");

        let (revision, _) = fixture.manager.sessions.read_settings(&id).await.unwrap();
        let inputs = SessionResources {
            agents: BTreeMap::from([(helper.clone(), "description='Review'\ninstructions='session-only sentinel'\nskills=[]\n".into())]),
            workflows: BTreeMap::from([(review.clone(), workflow_document("Session review"))]),
            ..SessionResources::default()
        };
        let request = |expected_revision, session_config, session_resources| Method::SessionInputsWrite {
            session_id: id.clone(),
            expected_revision,
            session_config,
            session_resources,
        };
        let MethodResult::SessionInputsWritten { revision: first } = call(
            &connection, 30, request(revision, None, Some(SessionInputChange::Replace(inputs.clone()))),
        ).await else { panic!("saved Session resources"); };
        assert_eq!(first, revision + 1);
        let _ = applied(&fixture, 0).await;
        assert_eq!(description(), "Session review");
        assert!(!ready(), "the Session Workflow cannot inherit its lower definition's grant");

        let config = SessionConfigContent("[agent]\nagents=['reviewer']\nworkflows=['review']\n[tool_deadline_policy]\nhard_deadline_ms=23456\n".into());
        let MethodResult::SessionInputsWritten { revision: second } = call(
            &connection, 31, request(first, Some(SessionInputChange::Replace(config.clone())), None),
        ).await else { panic!("saved Session config"); };
        assert_eq!(second, first + 1);
        let _ = applied(&fixture, 0).await;
        assert!(ready(), "the Session now grants the selected Workflow definition");
        assert_eq!(catalog().subagents().get(&helper).unwrap().instructions(), "session-only sentinel");
        let result = call(&connection, 32, Method::SettingsRead { session_id: id.clone() }).await;
        let MethodResult::Settings { projection } = &result else { panic!("Session settings"); };
        assert_eq!(projection.revision, second);
        let flags = projection.authored.as_ref().unwrap();
        assert!(flags.session_config && flags.session_resources);
        let encoded = serde_json::to_string(&result).unwrap();
        assert!(!encoded.contains("session-only sentinel"));
        assert!(!encoded.contains("23456"));

        let rejected = |request_id, call| {
            let connection = &connection;
            async move {
                use crate::app_server::protocol::{JsonRpcVersion, Request, RequestId, Response};
                let response = connection.handle_request(Request {
                    jsonrpc: JsonRpcVersion::V2,
                    id: RequestId::Integer(request_id),
                    call,
                }).await;
                let Response::Failure(failure) = response else { panic!("rejected Session inputs"); };
                failure.error.data.unwrap()
            }
        };
        let stale = rejected(33, request(first, Some(SessionInputChange::Clear), None)).await;
        assert_eq!(stale, ErrorData::StaleSettings { expected: first, actual: second });
        let invalid = rejected(34, request(
            second, Some(SessionInputChange::Replace(SessionConfigContent("[app_server]\n".into()))),
            Some(SessionInputChange::Clear),
        )).await;
        assert_eq!(invalid, ErrorData::SessionInputInvalid);
        let (unchanged, settings) = fixture.manager.sessions.read_settings(&id).await.unwrap();
        assert_eq!(unchanged, second);
        assert_eq!(settings.session_config, Some(config));
        assert_eq!(settings.session_resources, Some(inputs));

        let scoped = fixture.manager.configuration.source_bindings().clone();
        let catalog_on_disk = crate::local_runtime::session::SessionCatalog::open_existing(&scoped.runtime_root)
            .unwrap().unwrap();
        assert_eq!(catalog_on_disk.settings_revision(&id).unwrap(), second);
        assert_eq!(catalog_on_disk.lineage(&id, None).unwrap().1, settings);
        let source = SessionInputReference { session_id: id.clone(), revision: second };
        let MethodResult::SessionTransition { session: reused, .. } = call(
            &connection, 36, Method::SessionCreate {
                settings: crate::local_runtime::session::SessionPersistentState {
                    cwd: workspace.clone(), model: None, session_config: None, session_resources: None,
                },
                input_references: Some(SessionInputReferences {
                    session_config: Some(source.clone()), session_resources: Some(source.clone()),
                }),
                submission: None,
                dynamic_tools: Vec::new(),
            },
        ).await else { panic!("created from exact saved Session inputs"); };
        let (reused_revision, snapshot) = fixture.manager.sessions.read_settings(&reused.id).await.unwrap();
        assert_eq!(snapshot.session_config, settings.session_config);
        assert_eq!(snapshot.session_resources, settings.session_resources);
        let MethodResult::SessionInputsWritten { revision: cleared } = call(
            &connection, 35, request(second, Some(SessionInputChange::Clear), Some(SessionInputChange::Clear)),
        ).await else { panic!("cleared both fields"); };
        assert_eq!(cleared, second + 1);
        let stale_ref = rejected(37, Method::SessionInputsWrite {
            session_id: reused.id.clone(), expected_revision: reused_revision,
            session_config: Some(SessionInputChange::Reference(source)),
            session_resources: Some(SessionInputChange::Clear),
        }).await;
        assert_eq!(stale_ref, ErrorData::SessionInputSourceChanged);
        let (unchanged_revision, unchanged_snapshot) = fixture.manager.sessions.read_settings(&reused.id).await.unwrap();
        assert_eq!(unchanged_revision, reused_revision);
        assert_eq!(unchanged_snapshot, snapshot);
        let _ = applied(&fixture, 0).await;
        assert!(ready());
        assert_eq!(description(), "Workspace review");
        assert!(catalog().subagents().get(&helper).is_none());
        assert_eq!(fixture.provider.request_bodies(), [] as [String; 0]);

        let frozen = catalog();
        let mut settlement = SettlementReceiptWait::capture(&runtime);
        runtime.submit_inbound(input("request-A")).unwrap();
        fixture.gates[0].wait_entered().await;
        let admitted = runtime.configuration_cut().admitted_attempt.unwrap();
        let session_config = SessionConfigContent("[agent]\nworkflows=['review']\n".into());
        let session_resources = SessionResources {
            workflows: BTreeMap::from([(review.clone(), workflow_document("Replacement review"))]),
            ..SessionResources::default()
        };
        let MethodResult::SessionInputsWritten { revision: joint } = call(
            &connection,
            38,
            request(
                cleared,
                Some(SessionInputChange::Replace(session_config.clone())),
                Some(SessionInputChange::Replace(session_resources.clone())),
            ),
        ).await else { panic!("saved the complete authorized candidate"); };
        assert_eq!(joint, cleared + 1);
        let (persisted_revision, persisted) = fixture.manager.sessions.read_settings(&id).await.unwrap();
        assert_eq!(persisted_revision, joint);
        assert_eq!(persisted.session_config, Some(session_config));
        assert_eq!(persisted.session_resources, Some(session_resources));
        let staged = settled(&fixture, 0).await;
        assert_eq!(staged.units[&ApplyUnit::Capabilities], UnitApplication::Ready);
        assert_eq!(runtime.configuration_cut().admitted_attempt, Some(admitted));
        assert_eq!(catalog().revision(), frozen.revision());
        assert_eq!(description(), "Workspace review");

        fixture.gates[0].release();
        settlement.wait().await;
        assert_eq!(applied(&fixture, 0).await.units[&ApplyUnit::Capabilities], UnitApplication::Applied);
        assert!(ready());
        assert_eq!(description(), "Replacement review");
        runtime.submit_inbound(input("request-B")).unwrap();
        fixture.gates[1].wait_entered().await;
        fixture.gates[1].release();
        settlement.wait().await;
        let requests = fixture.provider.request_bodies();
        assert_eq!(requests.len(), 2);
        for (request, expected) in requests.iter().zip(["Workspace review", "Replacement review"]) {
            let body: serde_json::Value = serde_json::from_str(request).unwrap();
            let offered = body["tools"].as_array().unwrap().iter()
                .find(|tool| tool["function"]["name"] == "review").expect("review tool");
            assert_eq!(
                offered["function"]["description"],
                format!("{expected}\n\nPass business tool inputs inside the required arguments object. Runtime permission metadata belongs outside arguments."),
            );
        }
        assert_eq!(fixture.host.drain().await, [] as [String; 0]);
        assert!(client.wait().await.unwrap().success());
    })).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn client_package_watch_refreshes_permissions_and_nested_source_from_the_same_binding() {
    use crate::capabilities::ToolSourceId;
    use crate::runtime::permission::ResourcePermissionValue;

    Box::pin(bounded(async {
        let fixture = Fixture::with_session_count(None, 1).await;
        let root = fixture.workspaces[0].join(".agents/tools/watched");
        std::fs::create_dir_all(root.join("implementation/empty")).unwrap();
        std::fs::write(root.join("server.py"), "from implementation import tool\n").unwrap();
        let code = root.join("implementation/tool.py");
        std::fs::write(&code, "VERSION = 1\n").unwrap();
        std::fs::write(root.join("requirements.txt"), []).unwrap();
        let permission = root.join("permissions.toml");
        let write_permission = |value| {
            std::fs::write(
                &permission,
                format!("[permissions]\nplan='deny'\nautomatic='{value}'\nfull_access='allow'\n"),
            )
            .unwrap();
        };
        write_permission("ask");
        let mut client = fixture.start_execution(0).await;
        fixture.load(0).await.unwrap().unwrap();
        let id = &fixture.sessions[0].id;
        let runtime = fixture.manager.configuration_runtime(id).unwrap();
        let source = ToolSourceId::ManagedPython("watched".into());
        let observe = |snapshot: &crate::runtime::resources::RuntimeResourceSnapshot| {
            let package = snapshot.managed_python_catalog().packages()[&source]
                .as_ref()
                .unwrap();
            (
                package.source_digest().to_owned(),
                package.permissions().unwrap().automatic,
            )
        };
        let (registered, observed) = tokio::sync::oneshot::channel();
        fixture.manager.configuration.test_hooks.insert(
            "bound_configuration_watch_resynced",
            move || {
                registered.send(()).unwrap();
            },
        );
        let stop = tokio_util::sync::CancellationToken::new();
        let watch = tokio::spawn({
            let manager = fixture.manager.clone();
            let stop = stop.clone();
            async move { manager.run_source_watch(stop).await }
        });
        observed.await.unwrap();
        let (original, automatic) = observe(&runtime.runtime_resources());
        assert_eq!(automatic, ResourcePermissionValue::Ask);
        let mut changes = fixture.manager.configuration_changes();
        write_permission("deny");
        while observe(&runtime.runtime_resources()).1 != ResourcePermissionValue::Deny {
            changes.changed().await.unwrap();
        }
        let frozen = runtime.runtime_resources();
        assert_eq!(
            observe(&frozen),
            (original.clone(), ResourcePermissionValue::Deny)
        );

        let mut settlement = super::SettlementReceiptWait::capture(&runtime);
        runtime.submit_inbound(input("request-A")).unwrap();
        fixture.gates[0].wait_entered().await;
        let running = runtime
            .configuration_cut()
            .admitted_attempt
            .expect("the first request is admitted");
        let (resynced, recaptured) = tokio::sync::oneshot::channel();
        fixture.manager.configuration.test_hooks.insert(
            "bound_configuration_watch_resynced",
            move || {
                resynced.send(()).unwrap();
            },
        );
        std::fs::write(&code, "VERSION = 2\n").unwrap();
        recaptured.await.unwrap();
        let staged = settled(&fixture, 0).await;
        assert_eq!(
            staged.units[&ApplyUnit::Capabilities],
            UnitApplication::Ready
        );
        assert_eq!(
            runtime.configuration_cut().admitted_attempt,
            Some(running.clone())
        );
        assert_eq!(observe(&runtime.runtime_resources()), observe(&frozen));
        fixture.gates[0].release();
        settlement.wait().await;
        applied(&fixture, 0).await;
        let next = observe(&runtime.runtime_resources());
        assert_ne!(next.0, original);
        assert_eq!(next.1, ResourcePermissionValue::Deny);
        assert_eq!(observe(&frozen), (original, ResourcePermissionValue::Deny));

        runtime.submit_inbound(input("request-B")).unwrap();
        fixture.gates[1].wait_entered().await;
        assert_ne!(runtime.configuration_cut().admitted_attempt, Some(running));
        assert_eq!(observe(&runtime.runtime_resources()), next);
        fixture.gates[1].release();
        settlement.wait().await;
        assert_eq!(fixture.provider.request_bodies().len(), 2);
        stop.cancel();
        watch.await.unwrap().unwrap();
        assert_eq!(fixture.host.drain().await, [] as [String; 0]);
        assert!(client.wait().await.unwrap().success());
    }))
    .await;
}

#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "opt-in bwrap proof; run test-support/prove_execution_mount_view.sh"]
#[allow(clippy::too_many_lines)] // The same isolated Client serves both physical watch lanes.
async fn client_source_watch_reloads_guidance_and_skill_metadata_after_registration() {
    assert!(std::env::var_os("RUSTX_MOUNT_VIEW_PROOF").is_some());
    Box::pin(bounded(async {
        let fixture = Fixture::with_session_count(None, 1).await;
        let workspace = &fixture.workspaces[0];
        let mount = tempfile::tempdir().unwrap();
        let physical = mount.path().join("physical");
        std::fs::rename(workspace, &physical).unwrap();
        let instructions = physical.join("AGENTS.md");
        std::fs::write(&instructions, "before watch").unwrap();
        let collection = physical.join(".agents/skills");
        std::fs::create_dir_all(&collection).unwrap();
        let original_package = physical.join("skills-a");
        std::fs::create_dir(&original_package).unwrap();
        let replacement_package = physical.join("skills-b");
        std::fs::create_dir(&replacement_package).unwrap();
        let aliases = physical.join("aliases");
        std::fs::create_dir(&aliases).unwrap();
        let targets = physical.join("targets");
        std::fs::create_dir(&targets).unwrap();
        let selected = aliases.join("selected");
        std::os::unix::fs::symlink("../skills-a", &selected).unwrap();
        std::os::unix::fs::symlink("../../aliases/selected", collection.join("watched")).unwrap();
        let select = |target| {
            let next = aliases.join(".new");
            std::os::unix::fs::symlink(target, &next).unwrap();
            std::fs::rename(next, &selected).unwrap();
        };
        let rules = physical.join("rules");
        std::fs::create_dir(&rules).unwrap();
        let ignore = rules.join("skill-ignore");
        std::fs::write(&ignore, []).unwrap();
        std::os::unix::fs::symlink("../../rules/skill-ignore", collection.join(".gitignore"))
            .unwrap();
        let marker = original_package.join("SKILL.md");
        let write_marker = |marker: &std::path::Path, description| {
            std::fs::write(
                marker,
                format!(
                    "---\nname: watched\ndescription: {description}\n---\nBody remains on demand\n"
                ),
            )
            .unwrap();
        };
        write_marker(&marker, "Before watch");
        write_marker(
            &replacement_package.join("SKILL.md"),
            "Intermediate retarget",
        );
        let launcher = workspace_overlay_launcher(mount.path());
        assert!(!workspace.exists());
        let mut client = fixture.start_execution_via(0, Some(&launcher)).await;
        fixture.load(0).await.unwrap().unwrap();
        let id = fixture.sessions[0].id.clone();
        let runtime = fixture.manager.configuration_runtime(&id).unwrap();
        assert_eq!(
            runtime.runtime_resources().capability().skills().packages()[0].description(),
            "Before watch"
        );
        let (registered, observed) = tokio::sync::oneshot::channel();
        fixture.manager.configuration.test_hooks.insert(
            "bound_configuration_watch_resynced",
            move || {
                registered.send(()).unwrap();
            },
        );
        let manager = fixture.manager.clone();
        let stop = tokio_util::sync::CancellationToken::new();
        let worker_stop = stop.clone();
        let watcher = tokio::spawn(async move { manager.run_source_watch(worker_stop).await });
        observed.await.unwrap();
        let mut changed = fixture.manager.configuration_changes();
        std::fs::write(&instructions, "after watch").unwrap();
        write_marker(&marker, "After watch");
        loop {
            let resources = runtime.runtime_resources();
            if resources
                .capability()
                .skills()
                .packages()
                .iter()
                .any(|skill| skill.description() == "After watch")
                && resources
                    .project_context_files()
                    .iter()
                    .any(|file| file.content == "after watch")
            {
                break;
            }
            changed.changed().await.unwrap();
        }
        // Neither the public alias nor either marker changes at this cut.
        select("../skills-b");
        while runtime.runtime_resources().capability().skills().packages()[0].description()
            != "Intermediate retarget"
        {
            changed.changed().await.unwrap();
        }
        // Only the linked ignore file's physical target changes.
        std::fs::write(&ignore, "watched\n").unwrap();
        while !runtime
            .runtime_resources()
            .capability()
            .skills()
            .packages()
            .is_empty()
        {
            changed.changed().await.unwrap();
        }
        std::fs::write(&ignore, []).unwrap();
        while runtime
            .runtime_resources()
            .capability()
            .skills()
            .packages()
            .is_empty()
        {
            changed.changed().await.unwrap();
        }
        assert_eq!(
            runtime.runtime_resources().capability().skills().packages()[0].description(),
            "Intermediate retarget"
        );

        select("../targets/pending");
        while !runtime
            .runtime_resources()
            .capability()
            .skills()
            .packages()
            .is_empty()
        {
            changed.changed().await.unwrap();
        }
        let restored = targets.join("pending");
        let staged = mount.path().join("staging");
        std::fs::create_dir(&staged).unwrap();
        write_marker(&staged.join("SKILL.md"), "Missing target recovered");
        // The next change creates only the physical target, with both aliases untouched.
        std::fs::rename(staged, restored).unwrap();
        while runtime
            .runtime_resources()
            .capability()
            .skills()
            .packages()
            .first()
            .is_none_or(|skill| skill.description() != "Missing target recovered")
        {
            changed.changed().await.unwrap();
        }

        let endpoint = fixture.manager.execution_endpoint(&id).unwrap();
        let admission = endpoint.admission().unwrap().clone();
        let retired_lane = endpoint.control().unwrap();
        let (captured, stale_result) = tokio::sync::oneshot::channel();
        let (release, parked) = std::sync::mpsc::channel();
        fixture
            .manager
            .configuration
            .test_hooks
            .insert("bound_watch_captured", move || {
                captured.send(()).unwrap();
                let _ = parked.recv();
            });
        std::fs::write(&instructions, "stale before detach").unwrap();
        stale_result.await.unwrap();
        endpoint.detach_live_pair_for_test();
        assert!(endpoint.control().is_none());

        let (resynced, rebound) = tokio::sync::oneshot::channel();
        fixture.manager.configuration.test_hooks.insert(
            "bound_configuration_watch_resynced",
            move || {
                resynced.send(()).unwrap();
            },
        );
        std::fs::write(&instructions, "current after detach").unwrap();
        write_marker(&marker, "Current after detach");
        select("../skills-a");
        Fixture::admit_execution_pair(&endpoint).await;
        assert_eq!(endpoint.admission(), Some(&admission));
        assert!(!endpoint.control().unwrap().same_pair(&retired_lane));
        rebound.await.unwrap();
        loop {
            let resources = runtime.runtime_resources();
            if resources.capability().skills().packages()[0].description() == "Current after detach"
                && resources
                    .project_context_files()
                    .iter()
                    .any(|file| file.content == "current after detach")
            {
                break;
            }
            changed.changed().await.unwrap();
        }
        let (completed, old_finished) = tokio::sync::oneshot::channel();
        fixture.manager.configuration.test_hooks.insert(
            "bound_configuration_watch_resynced",
            move || {
                completed.send(()).unwrap();
            },
        );
        release.send(()).unwrap();
        old_finished.await.unwrap();
        let _ = settled(&fixture, 0).await;
        let resources = runtime.runtime_resources();
        assert_eq!(
            resources.capability().skills().packages()[0].description(),
            "Current after detach"
        );
        assert_eq!(
            resources
                .project_context_files()
                .iter()
                .map(|file| file.content.as_str())
                .collect::<Vec<_>>(),
            ["current after detach"]
        );
        assert_eq!(fixture.provider.request_bodies().len(), 0);
        assert!(!workspace.exists());
        stop.cancel();
        watcher.await.unwrap().unwrap();
        assert_eq!(fixture.host.drain().await, [] as [String; 0]);
        assert!(client.wait().await.unwrap().success());
    }))
    .await;
}

#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "opt-in bwrap proof; run test-support/prove_execution_mount_view.sh"]
async fn bound_skill_watch_keeps_the_failed_target_observable() {
    assert!(std::env::var_os("RUSTX_MOUNT_VIEW_PROOF").is_some());
    Box::pin(bounded(async {
        let fixture = Fixture::with_session_count(None, 1).await;
        let workspace = &fixture.workspaces[0];
        let mount = tempfile::tempdir().unwrap();
        let physical = mount.path().join("physical");
        std::fs::rename(workspace, &physical).unwrap();
        std::fs::write(physical.join("rustx.toml"), "[agent]\nskills = ['foo']\n").unwrap();
        let collection = physical.join(".agents/skills");
        std::fs::create_dir_all(&collection).unwrap();
        let original = physical.join("targets/a");
        let replacement = physical.join("targets/b");
        std::fs::create_dir_all(&original).unwrap();
        std::fs::create_dir_all(&replacement).unwrap();
        let write_marker = |directory: &std::path::Path, description| {
            std::fs::write(
                directory.join("SKILL.md"),
                format!("---\nname: foo\ndescription: {description}\n---\nBody\n"),
            ).unwrap();
        };
        write_marker(&original, "Original");
        std::fs::write(replacement.join("SKILL.md"), "broken frontmatter").unwrap();
        std::os::unix::fs::symlink("../../targets/a", collection.join("foo")).unwrap();
        let launcher = workspace_overlay_launcher(mount.path());
        let mut client = fixture.start_execution_via(0, Some(&launcher)).await;
        fixture.load(0).await.unwrap().unwrap();
        let id = &fixture.sessions[0].id;
        let runtime = fixture.manager.configuration_runtime(id).unwrap();
        let endpoint = fixture.manager.execution_endpoint(id).unwrap();
        let lane = endpoint.control().unwrap();
        let description = |snapshot: &crate::runtime::resources::RuntimeResourceSnapshot| {
            snapshot.capability().skills().packages()[0].description().to_owned()
        };

        let wait_for_resync = || {
            let (send, receive) = tokio::sync::oneshot::channel();
            fixture.manager.configuration.test_hooks.insert("bound_configuration_watch_resynced", move || {
                send.send(()).unwrap();
            });
            receive
        };
        let registered = wait_for_resync();
        let stop = tokio_util::sync::CancellationToken::new();
        let watcher = tokio::spawn({
            let manager = fixture.manager.clone();
            let stop = stop.clone();
            async move { manager.run_source_watch(stop).await }
        });
        registered.await.unwrap();

        let mut settlement = super::SettlementReceiptWait::capture(&runtime);
        runtime.submit_inbound(input("keep-original")).unwrap();
        fixture.gates[0].wait_entered().await;
        let frozen = runtime.runtime_resources();
        assert_eq!(description(&frozen), "Original");

        let (captured, retargeted) = tokio::sync::oneshot::channel();
        let (resume, parked) = std::sync::mpsc::channel();
        fixture.manager.configuration.test_hooks.insert("bound_configuration_watch_resynced", move || {
            captured.send(()).unwrap();
            let _ = parked.recv();
        });
        let next = physical.join(".next");
        std::os::unix::fs::symlink("../../targets/b", &next).unwrap();
        std::fs::rename(next, collection.join("foo")).unwrap();
        retargeted.await.unwrap();
        let failed = settled(&fixture, 0).await;
        assert!(matches!(&failed.units[&ApplyUnit::Capabilities], UnitApplication::Failed { diagnostic, .. }
            if diagnostic.contains("selected Skill is unavailable")));
        {
            let state = fixture.manager.applications.lock();
            let observed = state.bound_source_reads(id.as_str(), &lane).unwrap().skills.as_ref().unwrap();
            assert!(observed.complete);
            assert!(observed.paths.contains_key(&workspace.join("targets/b")));
            assert!(!observed.paths.contains_key(&workspace.join("targets/a")));
        }
        assert_eq!(description(&runtime.runtime_resources()), "Original");

        // Wait for native registration and its resync before editing the
        // replacement. Only the package marker changes after this point.
        let registered = wait_for_resync();
        resume.send(()).unwrap();
        registered.await.unwrap();
        assert_eq!(fixture.manager.configuration_application(id).unwrap(), failed);
        let recovered = wait_for_resync();
        write_marker(&replacement, "Recovered");
        recovered.await.unwrap();
        let prepared = settled(&fixture, 0).await;
        assert_ne!(prepared.desired, failed.desired);
        assert_eq!(prepared.units[&ApplyUnit::Capabilities], UnitApplication::Ready);
        assert_eq!(description(&runtime.runtime_resources()), "Original");
        assert_eq!(description(&frozen), "Original");
        assert!(endpoint.control().unwrap().same_pair(&lane));

        fixture.gates[0].release();
        settlement.wait().await;
        assert_eq!(applied(&fixture, 0).await.units[&ApplyUnit::Capabilities], UnitApplication::Applied);
        assert_eq!(description(&runtime.runtime_resources()), "Recovered");
        assert_eq!(description(&frozen), "Original");
        assert_eq!(fixture.provider.request_bodies().len(), 1);
        assert!(!workspace.exists());
        stop.cancel();
        watcher.await.unwrap().unwrap();
        assert_eq!(fixture.host.drain().await, [] as [String; 0]);
        assert!(client.wait().await.unwrap().success());
    })).await;
}

#[cfg(target_os = "linux")]
async fn isolate_bound_skill_workspace(
    fixture: &Fixture,
) -> (tempfile::TempDir, std::path::PathBuf, tokio::process::Child) {
    let mount = tempfile::tempdir().unwrap();
    let physical = mount.path().join("physical");
    std::fs::rename(&fixture.workspaces[0], &physical).unwrap();
    std::fs::write(physical.join("rustx.toml"), "[agent]\nskills = ['foo']\n").unwrap();
    let collection = physical.join(".agents/skills");
    std::fs::create_dir_all(&collection).unwrap();
    for target in ["a", "b"] {
        let package = physical.join("targets").join(target);
        std::fs::create_dir_all(&package).unwrap();
        std::fs::write(
            package.join("SKILL.md"),
            format!("---\nname: foo\ndescription: Skill {target}\n---\nBody\n"),
        )
        .unwrap();
    }
    std::os::unix::fs::symlink("../../targets/a", collection.join("foo")).unwrap();
    let launcher = workspace_overlay_launcher(mount.path());
    let child = fixture.start_execution_via(0, Some(&launcher)).await;
    fixture.load(0).await.unwrap().unwrap();
    (mount, physical, child)
}

#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "opt-in bwrap proof; run test-support/prove_execution_mount_view.sh"]
async fn session_input_commit_publishes_its_bound_skill_dependencies() {
    use super::super::protocol::{call, initialize};
    use crate::app_server::{
        connection::AppServerConnection,
        protocol::{Method, MethodResult},
    };
    use crate::local_runtime::session_inputs::{SessionConfigContent, SessionInputChange};

    assert!(std::env::var_os("RUSTX_MOUNT_VIEW_PROOF").is_some());
    Box::pin(bounded(async {
        let fixture = Fixture::with_session_count(None, 1).await;
        let (_mount, physical, mut client) = isolate_bound_skill_workspace(&fixture).await;
        let id = &fixture.sessions[0].id;
        let endpoint = fixture.manager.execution_endpoint(id).unwrap();
        let lane = endpoint.control().unwrap();
        let runtime = fixture.manager.configuration_runtime(id).unwrap();
        let description = || {
            runtime.runtime_resources().capability().skills().packages()[0]
                .description()
                .to_owned()
        };
        let hooks = &fixture.manager.configuration.test_hooks;
        let next_resync = || {
            let (tx, rx) = tokio::sync::oneshot::channel();
            hooks.insert("bound_configuration_watch_resynced", move || {
                tx.send(()).unwrap();
            });
            rx
        };
        let registered = next_resync();
        let stop = tokio_util::sync::CancellationToken::new();
        let watcher = tokio::spawn({
            let manager = fixture.manager.clone();
            let stop = stop.clone();
            async move { manager.run_source_watch(stop).await }
        });
        registered.await.unwrap();
        let check_paths = |expected, previous| {
            let state = fixture.manager.applications.lock();
            let paths = &state
                .bound_source_reads(id.as_str(), &lane)
                .unwrap()
                .skills
                .as_ref()
                .unwrap()
                .paths;
            assert!(paths.contains_key(&fixture.workspaces[0].join(expected)));
            assert!(!paths.contains_key(&fixture.workspaces[0].join(previous)));
        };
        assert_eq!(description(), "Skill a");
        check_paths("targets/a", "targets/b");

        let (captured, parked) = tokio::sync::oneshot::channel();
        let (resume, released) = std::sync::mpsc::channel();
        hooks.insert("bound_watch_captured", move || {
            captured.send(()).unwrap();
            let _ = released.recv();
        });
        let staged = physical.join(".next");
        std::os::unix::fs::symlink("../../targets/b", &staged).unwrap();
        std::fs::rename(staged, physical.join(".agents/skills/foo")).unwrap();
        parked.await.unwrap();

        let connection = AppServerConnection::new(fixture.host.clone());
        initialize(&connection).await;
        let (revision, _) = fixture.manager.sessions.read_settings(id).await.unwrap();
        let session_config =
            SessionConfigContent("[tool_deadline_policy]\nhard_deadline_ms=12345\n".into());
        let MethodResult::SessionInputsWritten { revision: written } = call(
            &connection,
            1,
            Method::SessionInputsWrite {
                session_id: id.clone(),
                expected_revision: revision,
                session_config: Some(SessionInputChange::Replace(session_config.clone())),
                session_resources: None,
            },
        )
        .await
        else {
            panic!("Session setting commit");
        };
        assert_eq!(written, revision + 1);
        let committed = applied(&fixture, 0).await.desired;
        assert_eq!(description(), "Skill b");
        check_paths("targets/b", "targets/a");

        // The old capture may settle, but its ticket no longer owns publication.
        let old = next_resync();
        resume.send(()).unwrap();
        old.await.unwrap();
        assert_eq!(
            fixture
                .manager
                .configuration_application(id)
                .unwrap()
                .desired,
            committed
        );
        let new_watch = next_resync();
        new_watch.await.unwrap();
        let updated = next_resync();
        std::fs::write(
            physical.join("targets/b/SKILL.md"),
            "---\nname: foo\ndescription: Updated b\n---\nBody\n",
        )
        .unwrap();
        updated.await.unwrap();
        assert_eq!(
            applied(&fixture, 0).await.units[&ApplyUnit::Capabilities],
            UnitApplication::Applied
        );
        assert_eq!(description(), "Updated b");
        let (persisted_revision, persisted) =
            fixture.manager.sessions.read_settings(id).await.unwrap();
        assert_eq!(persisted_revision, written);
        assert_eq!(persisted.session_config, Some(session_config));
        assert!(endpoint.control().unwrap().same_pair(&lane));
        assert!(!fixture.workspaces[0].exists());
        assert!(fixture.provider.request_bodies().is_empty());
        stop.cancel();
        watcher.await.unwrap().unwrap();
        assert_eq!(fixture.host.drain().await, [] as [String; 0]);
        assert!(client.wait().await.unwrap().success());
    }))
    .await;
}

#[cfg(target_os = "linux")]
#[tokio::test]
#[ignore = "opt-in bwrap proof; run test-support/prove_execution_mount_view.sh"]
async fn a_bound_source_rechecks_after_its_final_document_read_fails() {
    assert!(std::env::var_os("RUSTX_MOUNT_VIEW_PROOF").is_some());
    Box::pin(bounded(async {
        let fixture = Fixture::with_session_count(None, 1).await;
        let (mount, physical, mut client) = isolate_bound_skill_workspace(&fixture).await;
        let id = &fixture.sessions[0].id;
        let endpoint = fixture.manager.execution_endpoint(id).unwrap();
        let lane = endpoint.control().unwrap();
        let runtime = fixture.manager.configuration_runtime(id).unwrap();
        let hooks = &fixture.manager.configuration.test_hooks;
        let (registered, ready) = tokio::sync::oneshot::channel();
        hooks.insert("bound_configuration_watch_resynced", move || { registered.send(()).unwrap(); });
        let stop = tokio_util::sync::CancellationToken::new();
        let watcher = tokio::spawn({
            let manager = fixture.manager.clone();
            let stop = stop.clone();
            async move { manager.run_source_watch(stop).await }
        });
        ready.await.unwrap();
        let (failed_capture, failed) = tokio::sync::oneshot::channel();
        let (resume, parked) = std::sync::mpsc::channel();
        hooks.insert("bound_configuration_watch_resynced", move || {
            failed_capture.send(()).unwrap();
            let _ = parked.recv();
        });
        hooks.fail_once("bound_workspace_verification_unavailable");
        let staged = mount.path().join("AGENTS.md");
        std::fs::write(&staged, "Edited before a failed verification").unwrap();
        std::fs::rename(staged, physical.join("AGENTS.md")).unwrap();
        failed.await.unwrap();
        let failure = settled(&fixture, 0).await;
        assert!(matches!(&failure.units[&ApplyUnit::Instructions], UnitApplication::Failed { diagnostic } if diagnostic.contains("physical operation unavailable")));
        {
            let application = fixture.manager.applications.lock();
            assert!(application.bound_source_failed(id.as_str()));
            assert!(application.bound_source_reads(id.as_str(), &lane).unwrap().skills.as_ref().unwrap().complete);
        }
        assert!(runtime.runtime_resources().project_context_files().iter().all(|file| file.content != "Edited before a failed verification"));
        let (retrying, retry) = tokio::sync::oneshot::channel();
        let (read, continue_reading) = std::sync::mpsc::channel();
        hooks.insert("bound_source_recovery_due", move || {
            retrying.send(()).unwrap();
            let _ = continue_reading.recv();
        });
        tokio::time::pause();
        resume.send(()).unwrap();
        tokio::time::advance(std::time::Duration::from_secs(5)).await;
        retry.await.unwrap();
        tokio::time::resume();
        read.send(()).unwrap();
        let mut changed = fixture.manager.configuration_changes();
        loop {
            if let Some(view) = fixture.manager.configuration_application(id)
                && view.desired != failure.desired
                && view.units[&ApplyUnit::Instructions] == UnitApplication::Applied
            {
                break;
            }
            changed.changed().await.unwrap();
        }
        assert!(runtime.runtime_resources().project_context_files().iter().any(|file| file.content == "Edited before a failed verification"));
        assert!(endpoint.control().unwrap().same_pair(&lane));
        assert!(!fixture.workspaces[0].exists());
        assert!(fixture.provider.request_bodies().is_empty());
        stop.cancel();
        watcher.await.unwrap().unwrap();
        assert_eq!(fixture.host.drain().await, [] as [String; 0]);
        assert!(client.wait().await.unwrap().success());
    })).await;
}

#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "opt-in bwrap proof; run test-support/prove_execution_mount_view.sh"]
async fn replacing_a_skill_refreshes_the_catalog_while_native_watch_retirement_is_held() {
    use std::os::unix::fs::PermissionsExt as _;
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    Box::pin(bounded(async {
        let test_client = std::env::var_os("RUSTX_WATCH_RETIREMENT_PROOF_CLIENT").unwrap();
        let fixture = Fixture::with_session_count(None, 1).await;
        let workspace = &fixture.workspaces[0];
        let mount = tempfile::tempdir().unwrap();
        let physical = mount.path().join("physical");
        std::fs::rename(workspace, &physical).unwrap();
        let skills = physical.join(".agents/skills");
        std::fs::create_dir_all(&skills).unwrap();
        let target = physical.join("physical-skill");
        let replacement = physical.join("replacement-skill");
        for (folder, description) in [(&target, "Original"), (&replacement, "Replacement")] {
            std::fs::create_dir(folder).unwrap();
            std::fs::write(folder.join("SKILL.md"), format!("---\nname: watched\ndescription: {description}\n---\nBody\n")).unwrap();
        }
        std::os::unix::fs::symlink("../../physical-skill", skills.join("watched")).unwrap();
        std::os::unix::fs::symlink(test_client, mount.path().join("fixture-client")).unwrap();
        let retirement = tokio::net::UnixListener::bind(mount.path().join("retirement")).unwrap();
        let launcher = mount.path().join("client");
        std::fs::write(&launcher, r#"#!/bin/sh
set -eu
bin=${0%/*}
unset RUSTX_CLIENT_DOMAIN_STAGING
export RUSTX_TEST_WATCH_RETIREMENT_PATH=physical-skill
export RUSTX_TEST_WATCH_RETIREMENT_SOCKET="$bin/retirement"
exec /usr/bin/bwrap --die-with-parent --ro-bind / / --dev /dev --tmpfs "${4%/*}" --bind "$bin/physical" "$4" -- "$bin/fixture-client" "$@"
"#).unwrap();
        std::fs::set_permissions(&launcher, std::fs::Permissions::from_mode(0o700)).unwrap();
        let mut client = fixture.start_execution_via(0, Some(&launcher)).await;
        fixture.load(0).await.unwrap().unwrap();
        let runtime = fixture.manager.configuration_runtime(&fixture.sessions[0].id).unwrap();
        let description = || runtime.runtime_resources().capability().skills().packages()[0].description().to_owned();
        assert_eq!(description(), "Original");
        let (registered, ready) = tokio::sync::oneshot::channel();
        fixture.manager.configuration.test_hooks.insert("bound_configuration_watch_resynced", move || registered.send(()).unwrap());
        let stop = tokio_util::sync::CancellationToken::new();
        let watcher = tokio::spawn({
            let manager = fixture.manager.clone();
            let stop = stop.clone();
            async move { manager.run_source_watch(stop).await }
        });
        ready.await.unwrap();
        let (invalidated, noticed) = tokio::sync::oneshot::channel();
        fixture.manager.configuration.test_hooks.insert("bound_watch_retiring", move || invalidated.send(()).unwrap());
        let mut changes = fixture.manager.configuration_changes();
        // Both the Skills collection and its symbolic link stay unchanged.
        rustix::fs::renameat_with(rustix::fs::CWD, &target, rustix::fs::CWD,
            &replacement, rustix::fs::RenameFlags::EXCHANGE).unwrap();
        let (mut held, _) = retirement.accept().await.unwrap();
        let mut marker = [0; 8];
        held.read_exact(&mut marker).await.unwrap();
        assert_eq!(&marker, b"retiring");
        noticed.await.unwrap();
        while description() != "Replacement" {
            changes.changed().await.unwrap();
        }
        let endpoint = fixture.manager.execution_endpoint(&fixture.sessions[0].id).unwrap();
        endpoint.control().unwrap().health().unwrap();
        assert_eq!(held.try_read(&mut [0]).unwrap_err().kind(), std::io::ErrorKind::WouldBlock);
        held.write_all(b"r").await.unwrap();
        drop(held);
        // The refreshed catalog can register another watcher of the same path.
        let retirements = tokio::spawn(async move {
            while let Ok((mut gate, _)) = retirement.accept().await {
                let _ = gate.write_all(b"r").await;
            }
        });
        stop.cancel();
        watcher.await.unwrap().unwrap();
        assert!(!workspace.exists());
        assert_eq!(fixture.host.drain().await, [] as [String; 0]);
        assert!(client.wait().await.unwrap().success());
        retirements.abort();
    })).await;
}

#[cfg(all(target_os = "linux", feature = "mcp-fixture"))]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "opt-in bwrap proof; run test-support/prove_execution_mount_view.sh"]
async fn bound_workspace_mcp_catalog_uses_client_definitions_and_server_network() {
    use super::super::protocol::{call, initialize};
    use crate::app_server::{
        connection::AppServerConnection,
        protocol::{Method, MethodResult},
    };
    use crate::local_runtime::{configuration::settings::SourceTarget, mcp_probe::McpProbeOutcome};
    use crate::tools::mcp::{
        catalog::CatalogKind,
        fixture::streamable_http::{HttpFixture, HttpFixtureControl},
    };
    assert!(std::env::var_os("RUSTX_MOUNT_VIEW_PROOF").is_some());
    Box::pin(bounded(async {
        let fixture = Fixture::with_session_count(None, 1).await;
        let workspace = &fixture.workspaces[0];
        let physical = tempfile::tempdir().unwrap();
        let overlay = physical.path().join("physical");
        std::fs::rename(workspace, &overlay).unwrap();
        let control = HttpFixtureControl::default();
        let service = HttpFixture::start(control.clone()).await;
        let document = format!("[mcp_servers.inventory]\nurl = {:?}\n", service.endpoint);
        let resources = overlay.join(".agents");
        std::fs::create_dir_all(&resources).unwrap();
        std::fs::write(resources.join("mcp.toml"), &document).unwrap();
        let launcher = workspace_overlay_launcher(physical.path());
        let mut client = fixture.start_execution_via(0, Some(&launcher)).await;
        let connection = AppServerConnection::new(fixture.host.clone());
        initialize(&connection).await;
        let MethodResult::McpSource {
            authority,
            source: Some(source),
        } = call(
            &connection,
            98,
            Method::McpSourceRead {
                target: SourceTarget::Workspace {
                    directory: workspace.clone(),
                },
                execution_session: fixture.sessions[0].id.clone(),
            },
        )
        .await
        else {
            panic!("bound Workspace MCP source");
        };
        assert_eq!(
            authority,
            crate::app_server::protocol::WorkspaceCatalogAuthority::Client
        );
        assert_eq!(
            source.revision,
            crate::local_runtime::settings::revision(Some(document.as_bytes()))
        );
        assert_eq!(source.path, workspace.join(".agents/mcp.toml"));
        assert_eq!(
            source
                .authored
                .unwrap()
                .into_keys()
                .map(|id| id.to_string())
                .collect::<Vec<_>>(),
            ["inventory"],
        );
        let revision = source.revision;
        assert!(!workspace.exists());
        std::fs::create_dir_all(workspace.join(".agents")).unwrap();
        let decoy = b"[mcp_servers.decoy]\nurl='http://127.0.0.1:1'\n";
        std::fs::write(workspace.join(".agents/mcp.toml"), decoy).unwrap();
        assert_ne!(
            revision,
            crate::local_runtime::settings::revision(Some(decoy))
        );
        let MethodResult::McpSource {
            authority,
            source: Some(source),
        } = call(
            &connection,
            100,
            Method::McpSourceRead {
                target: SourceTarget::Workspace {
                    directory: workspace.clone(),
                },
                execution_session: fixture.sessions[0].id.clone(),
            },
        )
        .await
        else {
            panic!("Client source with a Server decoy");
        };
        assert_eq!(
            authority,
            crate::app_server::protocol::WorkspaceCatalogAuthority::Client
        );
        assert_eq!(source.revision, revision);
        assert!(
            source
                .authored
                .unwrap()
                .keys()
                .all(|id| id.as_str() != "decoy")
        );
        let MethodResult::McpProbe { result: probe } = call(
            &connection,
            101,
            Method::McpProbe {
                target: SourceTarget::Workspace {
                    directory: workspace.clone(),
                },
                execution_session: Some(fixture.sessions[0].id.clone()),
                id: crate::runtime::identity::McpServerId::new("inventory"),
                expected_revision: revision.clone(),
            },
        )
        .await
        else {
            panic!("Client revision probe");
        };
        assert_eq!(probe.outcome, McpProbeOutcome::Reachable);
        assert_eq!(probe.revision, revision);
        assert_eq!(control.catalog_reads(), 1);
        let MethodResult::McpProbe { result } = call(
            &connection,
            99,
            Method::McpCatalog {
                target: SourceTarget::Workspace {
                    directory: workspace.clone(),
                },
                execution_session: Some(fixture.sessions[0].id.clone()),
                id: crate::runtime::identity::McpServerId::new("inventory"),
                expected_revision: revision.clone(),
                kind: CatalogKind::Tools,
            },
        )
        .await
        else {
            panic!("bounded Workspace MCP inventory");
        };
        assert_eq!(result.outcome, McpProbeOutcome::Reachable);
        assert_eq!(result.revision, revision);
        assert_eq!(result.page.unwrap().tools.len(), 5);
        assert_eq!(control.catalog_reads(), 2);
        assert_eq!(control.accepted_calls(), 0);
        assert_eq!(
            std::fs::read(workspace.join(".agents/mcp.toml")).unwrap(),
            decoy
        );
        assert_eq!(fixture.host.drain().await, [] as [String; 0]);
        assert!(client.wait().await.unwrap().success());
        service.shutdown().await;
    }))
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(clippy::too_many_lines)]
async fn workspace_catalog_authority_separates_unclaimed_authoring_claimed_unavailable_and_live_client()
 {
    use super::super::protocol::{call, initialize};
    use crate::app_server::{
        connection::AppServerConnection,
        protocol::{
            ErrorData, JsonRpcVersion, Method, MethodResult, Request, RequestId, Response,
            WorkspaceCatalogAuthority,
        },
    };
    use crate::local_runtime::configuration::settings::SourceTarget;
    use std::path::Path;
    Box::pin(bounded(async {
        let fixture = Fixture::with_session_count(None, 2).await;
        let workspace = fixture.workspaces[0].clone();
        std::fs::create_dir_all(workspace.join(".agents/skills/authored-skill")).unwrap();
        std::fs::write(
            workspace.join(".agents/mcp.toml"),
            "[mcp_servers.authored]\nurl = \"http://127.0.0.1:9\"\n",
        )
        .unwrap();
        std::fs::write(
            workspace.join(".agents/skills/authored-skill/SKILL.md"),
            "---\nname: authored-skill\ndescription: Authored skill\n---\nBody\n",
        )
        .unwrap();
        let connection = AppServerConnection::new(fixture.host.clone());
        initialize(&connection).await;

        let mcp_read = |session_id: &SessionId, dir: &Path| Method::McpSourceRead {
            target: SourceTarget::Workspace {
                directory: dir.to_path_buf(),
            },
            execution_session: session_id.clone(),
        };
        let skills_list = |session_id: &SessionId, dir: &Path| Method::SkillsList {
            target: SourceTarget::Workspace {
                directory: dir.to_path_buf(),
            },
            execution_session: Some(session_id.clone()),
            offset: 0,
            limit: 32,
            expected_revision: None,
        };

        // 1. Confirmed unclaimed Session with no Client -> Authoring authority.
        let MethodResult::McpSource { authority, source } = call(
            &connection,
            1,
            mcp_read(&fixture.sessions[0].id, &workspace),
        )
        .await
        else {
            panic!("expected McpSource for unclaimed session");
        };
        assert_eq!(authority, WorkspaceCatalogAuthority::Authoring);
        assert!(source.is_none());

        let MethodResult::Skills { page, authority } = call(
            &connection,
            2,
            skills_list(&fixture.sessions[0].id, &workspace),
        )
        .await
        else {
            panic!("expected Skills for unclaimed session");
        };
        assert_eq!(authority, WorkspaceCatalogAuthority::Authoring);
        assert!(page.data.iter().any(|s| s.catalog.name == "authored-skill"));

        // 2. Claimed Session whose Client is unavailable -> ExecutionUnavailable (no fallback).
        let _reserved = fixture
            .manager
            .reserve_local_execution(&fixture.sessions[0].id)
            .await
            .unwrap();
        let response = connection
            .handle_request(Request {
                jsonrpc: JsonRpcVersion::V2,
                id: RequestId::Integer(3),
                call: mcp_read(&fixture.sessions[0].id, &workspace),
            })
            .await;
        let Response::Failure(failure) = response else {
            panic!("claimed Session without live Client must refuse McpSourceRead");
        };
        assert_eq!(failure.error.data, Some(ErrorData::ExecutionUnavailable));

        let response = connection
            .handle_request(Request {
                jsonrpc: JsonRpcVersion::V2,
                id: RequestId::Integer(4),
                call: skills_list(&fixture.sessions[0].id, &workspace),
            })
            .await;
        let Response::Failure(failure) = response else {
            panic!("claimed Session without live Client must refuse SkillsList");
        };
        assert_eq!(failure.error.data, Some(ErrorData::ExecutionUnavailable));

        // Unsettled durable RecoveryFence without an in-memory slot also refuses fallback.
        let (fenced_binding, _) =
            rustx_execution_server::Binding::reserve(rustx_execution_server::Reservation {
                session: rustx_execution_protocol::SessionId::new(fixture.sessions[1].id.as_str()),
                environment: rustx_execution_protocol::EnvironmentId::new("env"),
                instance: rustx_execution_protocol::ClientInstanceId::new("lost-instance"),
                generation: rustx_execution_protocol::Generation::new(std::num::NonZeroU64::MIN),
                workspace: fixture.workspaces[1].clone(),
                required_capabilities: std::collections::BTreeSet::new(),
            })
            .unwrap();
        let durable_fence = fenced_binding
            .retain_recovery_fence(fixture.manager.configuration.runtime_root())
            .unwrap();
        let response = connection
            .handle_request(Request {
                jsonrpc: JsonRpcVersion::V2,
                id: RequestId::Integer(5),
                call: mcp_read(&fixture.sessions[1].id, &fixture.workspaces[1]),
            })
            .await;
        let Response::Failure(failure) = response else {
            panic!("durable recovery fence must refuse McpSourceRead");
        };
        assert_eq!(failure.error.data, Some(ErrorData::ExecutionUnavailable));
        let response = connection
            .handle_request(Request {
                jsonrpc: JsonRpcVersion::V2,
                id: RequestId::Integer(6),
                call: skills_list(&fixture.sessions[1].id, &fixture.workspaces[1]),
            })
            .await;
        let Response::Failure(failure) = response else {
            panic!("durable recovery fence must refuse SkillsList");
        };
        assert_eq!(failure.error.data, Some(ErrorData::ExecutionUnavailable));
        drop(durable_fence);
        let _ = fixture.host.drain().await;
    }))
    .await;
}

fn python_fallback_packages(
    workspace: &std::path::Path,
) -> (std::path::PathBuf, std::path::PathBuf) {
    let tools = workspace.join(".agents/tools");
    let healthy = tools.join("healthy-pkg");
    let incomplete = tools.join("incomplete-pkg");
    for root in [&healthy, &incomplete] {
        std::fs::create_dir_all(root).unwrap();
        std::fs::write(root.join("server.py"), "VERSION = 1\n").unwrap();
        std::fs::write(root.join("requirements.txt"), []).unwrap();
    }
    for index in 0..=rustx_execution_protocol::mcp::MAX_SOURCE_WATCH_DIRECTORIES {
        std::fs::create_dir_all(incomplete.join(format!("d{index:03}"))).unwrap();
    }
    std::fs::write(incomplete.join("d000/helper.py"), "HELPER = 1\n").unwrap();
    (healthy, incomplete)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn selective_python_watch_fallback_skips_healthy_siblings() {
    python_watch_fallback(false).await;
}

#[tokio::test]
async fn selective_python_watch_fallback_recovers_after_configuration_read_failure() {
    python_watch_fallback(true).await;
}

#[allow(clippy::too_many_lines)]
async fn python_watch_fallback(fail_configuration_read: bool) {
    use crate::capabilities::ToolSourceId;
    use rustx_execution_protocol::{
        Bootstrap, BootstrapEndpoint, ClientReply, ClientRequest, ControlHelloReply,
        DataHelloReply, PeerHello, ReadError,
        mcp::{PythonSourceReply, PythonSourceRequest},
        transport::{read_record, write_record},
    };
    use std::collections::BTreeMap;
    use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
    use std::sync::Mutex;

    Box::pin(bounded(async {
        let fixture = Fixture::with_session_count(None, 1).await;
        let (healthy, incomplete) = python_fallback_packages(&fixture.workspaces[0]);

        let endpoint = fixture
            .manager
            .reserve_local_execution(&fixture.sessions[0].id)
            .await
            .unwrap();
        let scratch = tempfile::tempdir().unwrap();
        std::fs::set_permissions(scratch.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let proxy_path = scratch.path().canonicalize().unwrap().join("proxy.sock");
        let proxy = std::os::unix::net::UnixListener::bind(&proxy_path).unwrap();
        let mut bootstrap: Bootstrap =
            read_record(&mut std::fs::File::open(endpoint.bootstrap_path()).unwrap()).unwrap();
        let BootstrapEndpoint::Unix {
            socket_path: server_socket,
        } = bootstrap.endpoint
        else {
            panic!("local execution endpoint");
        };
        bootstrap.endpoint = BootstrapEndpoint::Unix {
            socket_path: proxy_path,
        };
        let bundle = scratch.path().join("binding");
        let mut bundle_file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&bundle)
            .unwrap();
        write_record(&mut bundle_file, &bootstrap).unwrap();
        drop(bundle_file);

        let binary = std::env::current_exe()
            .unwrap()
            .with_file_name("rustx-execution-client");
        let mut client = tokio::process::Command::new(binary)
            .arg("--binding-file")
            .arg(bundle.canonicalize().unwrap())
            .arg("--workspace")
            .arg(endpoint.workspace())
            .current_dir("/")
            .env_clear()
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .unwrap();

        let crate::local_runtime::session_runtime_manager::ExecutionListener::Unix(server_listener) =
            endpoint.listener()
        else {
            panic!("Unix listener");
        };
        let server_listener = server_listener.try_clone().unwrap();
        let capture_counts = Arc::new(Mutex::new(BTreeMap::<String, usize>::new()));
        let counts_wire = capture_counts.clone();
        let (arm_failure, arm_wire) = std::sync::mpsc::channel();
        let (injected, mut injected_wire) = tokio::sync::oneshot::channel();
        let (fail_next_read, requested_failure) = std::sync::mpsc::channel();
        let (observed_read, mut read_observation) = tokio::sync::mpsc::unbounded_channel();
        let endpoint_ref = endpoint.clone();
        let (live_tx, live_rx) = tokio::sync::oneshot::channel();
        let proxy_join = std::thread::spawn(move || {
            let (mut data_peer, _) = proxy.accept().unwrap();
            let mut data_server = std::os::unix::net::UnixStream::connect(&server_socket).unwrap();
            let (server_data_accepted, _) = server_listener.accept().unwrap();
            let endpoint_data = endpoint_ref.clone();
            let admit_data =
                std::thread::spawn(move || endpoint_data.admit_local_stream(server_data_accepted));
            let hello: PeerHello = read_record(&mut data_peer).unwrap();
            write_record(&mut data_server, &hello).unwrap();
            let reply: DataHelloReply = read_record(&mut data_server).unwrap();
            write_record(&mut data_peer, &reply).unwrap();
            admit_data.join().unwrap().unwrap();

            let (mut ctrl_peer, _) = proxy.accept().unwrap();
            let mut ctrl_server = std::os::unix::net::UnixStream::connect(&server_socket).unwrap();
            let (server_ctrl_accepted, _) = server_listener.accept().unwrap();
            let endpoint_ctrl = endpoint_ref.clone();
            let admit_ctrl =
                std::thread::spawn(move || endpoint_ctrl.admit_local_stream(server_ctrl_accepted));
            let hello: PeerHello = read_record(&mut ctrl_peer).unwrap();
            write_record(&mut ctrl_server, &hello).unwrap();
            let reply: ControlHelloReply = read_record(&mut ctrl_server).unwrap();
            write_record(&mut ctrl_peer, &reply).unwrap();
            admit_ctrl.join().unwrap().unwrap();
            let _ = live_tx.send(());

            let exchange_data = std::thread::spawn(move || {
                let mut last_recheck = 0;
                let mut affected_capture = None;
                let mut captured_digest = None;
                let mut injected = Some(injected);
                while let Ok(req) = read_record::<ClientRequest>(&mut data_server) {
                    if let ClientRequest::PythonSource(PythonSourceRequest::Capture {
                        name, key, ..
                    }) = &req
                    {
                        *counts_wire.lock().unwrap().entry(name.clone()).or_insert(0) += 1;
                        if name == "incomplete-pkg" && arm_wire.try_recv().is_ok() {
                            affected_capture = Some(key.clone());
                        }
                    }
                    if write_record(&mut data_peer, &req).is_err() {
                        break;
                    }
                    let Ok(mut reply) = read_record::<ClientReply>(&mut data_peer) else {
                        break;
                    };
                    match (&req, &reply) {
                        (
                            ClientRequest::PythonSource(
                                PythonSourceRequest::Capture { key, .. } | PythonSourceRequest::State { key },
                            ),
                            ClientReply::PythonSource(PythonSourceReply::Captured { digest, .. }),
                        ) if affected_capture.as_ref() == Some(key) => {
                            affected_capture = None;
                            captured_digest = Some(digest.clone());
                        }
                        (ClientRequest::ReadRange { relative_path, .. }, _) if relative_path == std::path::Path::new("rustx.toml") =>
                        {
                            if let Some(digest) = captured_digest.take() {
                                reply = ClientReply::Error { kind: ReadError::Unavailable };
                                injected.take().unwrap().send(digest).unwrap();
                            } else {
                                if let Ok(retry) = requested_failure.try_recv() {
                                    last_recheck = retry;
                                    if retry != 0 {
                                        observed_read.send(retry).unwrap();
                                    }
                                }
                                if last_recheck != 0 {
                                    reply = ClientReply::Error { kind: ReadError::Unavailable };
                                }
                            }
                        }
                        _ => {}
                    }
                    if write_record(&mut data_server, &reply).is_err() {
                        break;
                    }
                }
            });
            let mut ctrl_peer_rx = ctrl_peer.try_clone().unwrap();
            let mut ctrl_server_tx = ctrl_server.try_clone().unwrap();
            let ctrl_s2c = std::thread::spawn(move || {
                let _ = std::io::copy(&mut ctrl_server, &mut ctrl_peer);
            });
            let ctrl_c2s = std::thread::spawn(move || {
                let _ = std::io::copy(&mut ctrl_peer_rx, &mut ctrl_server_tx);
            });
            let _ = exchange_data.join();
            let _ = ctrl_s2c.join();
            let _ = ctrl_c2s.join();
        });
        live_rx.await.unwrap();

        fixture.load(0).await.unwrap().unwrap();
        let id = &fixture.sessions[0].id;
        let runtime = fixture.manager.configuration_runtime(id).unwrap();
        let healthy_source = ToolSourceId::ManagedPython("healthy-pkg".into());
        let incomplete_source = ToolSourceId::ManagedPython("incomplete-pkg".into());
        let digest = |snapshot: &crate::runtime::resources::RuntimeResourceSnapshot,
                      source: &ToolSourceId| {
            snapshot.managed_python_catalog().packages()[source]
                .as_ref()
                .unwrap()
                .source_digest()
                .to_owned()
        };
        let initial_healthy = digest(&runtime.runtime_resources(), &healthy_source);
        let initial_incomplete = digest(&runtime.runtime_resources(), &incomplete_source);
        assert_eq!(capture_counts.lock().unwrap().get("healthy-pkg"), Some(&1));
        assert_eq!(
            capture_counts.lock().unwrap().get("incomplete-pkg"),
            Some(&1)
        );

        let (registered, observed) = tokio::sync::oneshot::channel();
        fixture.manager.configuration.test_hooks.insert(
            "bound_configuration_watch_resynced",
            move || {
                registered.send(()).unwrap();
            },
        );
        let stop = tokio_util::sync::CancellationToken::new();
        let watch = tokio::spawn({
            let manager = fixture.manager.clone();
            let stop = stop.clone();
            async move { manager.run_source_watch(stop).await }
        });
        observed.await.unwrap();
        // Initial watch registration changes the watch plan and performs one full resync.
        let healthy_after_register = *capture_counts.lock().unwrap().get("healthy-pkg").unwrap();
        let incomplete_after_register = *capture_counts
            .lock()
            .unwrap()
            .get("incomplete-pkg")
            .unwrap();
        assert_eq!(healthy_after_register, 2);
        assert_eq!(incomplete_after_register, 2);

        let watch_owner = endpoint.control().unwrap();
        if fail_configuration_read {
            let unmonitored_file = incomplete.join("d000/helper.py");
            fixture.manager.configuration.test_hooks.insert("before_bound_python_fallback_read", move || {
                std::fs::write(unmonitored_file, "HELPER = 2\n").unwrap();
                arm_failure.send(()).unwrap();
            });
        }
        let (resume, parked) = std::sync::mpsc::channel();
        let (fallback_tx, fallback_rx) = tokio::sync::oneshot::channel();
        fixture.manager.configuration.test_hooks.insert(
            "bound_python_fallback_checked",
            move || {
                fallback_tx.send(()).unwrap();
                let _ = parked.recv();
            },
        );
        fallback_rx.await.unwrap();
        assert_eq!(
            capture_counts.lock().unwrap().get("healthy-pkg"),
            Some(&healthy_after_register)
        );
        assert_eq!(
            capture_counts.lock().unwrap().get("incomplete-pkg"),
            Some(&(incomplete_after_register + 1))
        );
        assert_eq!(
            digest(&runtime.runtime_resources(), &healthy_source),
            initial_healthy
        );
        assert_eq!(
            digest(&runtime.runtime_resources(), &incomplete_source),
            initial_incomplete
        );

        if fail_configuration_read {
            let captured = injected_wire.try_recv().expect("one configuration read failed after the updated package was captured");
            assert_ne!(captured, initial_incomplete);
            assert!(fixture.manager.applications.lock().latest_bound_source(id.as_str()).is_none());
            assert!(endpoint.control().unwrap().same_pair(&watch_owner));
            let failed = settled(&fixture, 0).await;
            assert!(matches!(failed.units[&ApplyUnit::Capabilities], UnitApplication::Failed { .. }));
            let mut resume = resume;
            for round in 1..=2 {
                // Drive the periodic check against a real data exchange. The
                // clock only selects the due tick; IPC runs in real time.
                tokio::time::pause();
                let (rechecked, next_check) = tokio::sync::oneshot::channel();
                let (next_resume, park) = std::sync::mpsc::channel();
                let (started, started_read) = tokio::sync::oneshot::channel();
                let (go, allow_read) = std::sync::mpsc::channel();
                fixture.manager.configuration.test_hooks.insert("before_bound_source_read", move || {
                    started.send(()).unwrap();
                    let _ = allow_read.recv();
                });
                fixture.manager.configuration.test_hooks.insert("bound_python_fallback_checked", move || {
                    rechecked.send(()).unwrap();
                    let _ = park.recv();
                });
                tokio::time::advance(std::time::Duration::from_secs(5)).await;
                resume.send(()).unwrap();
                started_read.await.unwrap();
                tokio::time::resume();
                fail_next_read.send(round).unwrap();
                go.send(()).unwrap();
                assert_eq!(read_observation.recv().await.unwrap(), round);
                next_check.await.unwrap();
                let view = fixture.manager.configuration_application(id).unwrap();
                assert_eq!(view, failed);
                assert_eq!(digest(&runtime.runtime_resources(), &incomplete_source), initial_incomplete);
                resume = next_resume;
            }
            fail_next_read.send(0).unwrap();
            let (rechecked, next_check) = tokio::sync::oneshot::channel();
            fixture.manager.configuration.test_hooks.insert("bound_python_fallback_checked", move || {
                rechecked.send(()).unwrap();
            });
            resume.send(()).unwrap();
            next_check.await.unwrap();
            stop.cancel();
            watch.await.unwrap().unwrap();
            let adopted = applied(&fixture, 0).await;
            assert_eq!(adopted.units[&ApplyUnit::Capabilities], UnitApplication::Applied);
            assert_eq!(digest(&runtime.runtime_resources(), &incomplete_source), captured);
            assert_eq!(digest(&runtime.runtime_resources(), &healthy_source), initial_healthy);
            assert!(endpoint.control().unwrap().same_pair(&watch_owner));
            assert_eq!(capture_counts.lock().unwrap()["healthy-pkg"], healthy_after_register + 1);
            assert_eq!(capture_counts.lock().unwrap()["incomplete-pkg"], incomplete_after_register + 2);
            assert_eq!(fixture.host.drain().await, [] as [String; 0]);
            assert!(client.wait().await.unwrap().success());
            proxy_join.join().unwrap();
            return;
        }
        resume.send(()).unwrap();

        let mut settlement = super::SettlementReceiptWait::capture(&runtime);
        runtime.submit_inbound(input("request-A")).unwrap();
        fixture.gates[0].wait_entered().await;
        let running = runtime.configuration_cut().admitted_attempt.unwrap();
        let frozen = runtime.runtime_resources();
        let (resynced_tx, resynced_rx) = tokio::sync::oneshot::channel();
        fixture.manager.configuration.test_hooks.insert(
            "bound_configuration_watch_resynced",
            move || {
                resynced_tx.send(()).unwrap();
            },
        );
        std::fs::write(healthy.join("server.py"), "VERSION = 2\n").unwrap();
        resynced_rx.await.unwrap();
        let staged = settled(&fixture, 0).await;
        assert_eq!(staged.units[&ApplyUnit::Capabilities], UnitApplication::Ready);
        stop.cancel();
        watch.await.unwrap().unwrap();
        let desired = |source: &ToolSourceId| {
            let application = fixture.manager.applications.lock();
            let (capture, _) = application.latest_bound_source(id.as_str()).unwrap();
            capture.managed_python.packages()[source].as_ref().unwrap().source_digest().to_owned()
        };
        let healthy_update = desired(&healthy_source);
        assert_ne!(healthy_update, initial_healthy);
        assert_eq!(desired(&incomplete_source), initial_incomplete);
        let before = capture_counts.lock().unwrap().clone();

        std::fs::write(incomplete.join("d000/helper.py"), "HELPER = 2\n").unwrap();
        assert!(tokio::task::spawn_blocking({
            let manager = fixture.manager.clone();
            let id = id.clone();
            let endpoint = endpoint.clone();
            let incomplete_source = incomplete_source.clone();
            move || manager.observe_incomplete_bound_python(&id, &endpoint, endpoint.control().unwrap(), &std::collections::BTreeSet::from([incomplete_source]))
        }).await.unwrap());
        let staged = settled(&fixture, 0).await;
        assert_eq!(staged.units[&ApplyUnit::Capabilities], UnitApplication::Ready);
        assert_eq!(desired(&healthy_source), healthy_update);
        let incomplete_update = desired(&incomplete_source);
        assert_ne!(incomplete_update, initial_incomplete);
        {
            let counts = capture_counts.lock().unwrap();
            assert_eq!(counts["healthy-pkg"], before["healthy-pkg"]);
            assert_eq!(counts["incomplete-pkg"], before["incomplete-pkg"] + 1);
        }
        assert_eq!(runtime.configuration_cut().admitted_attempt, Some(running.clone()));
        assert_eq!(digest(&runtime.runtime_resources(), &healthy_source), initial_healthy);
        assert_eq!(digest(&runtime.runtime_resources(), &incomplete_source), initial_incomplete);
        fixture.gates[0].release();
        settlement.wait().await;
        applied(&fixture, 0).await;
        runtime.submit_inbound(input("request-B")).unwrap();
        fixture.gates[1].wait_entered().await;
        assert_ne!(runtime.configuration_cut().admitted_attempt, Some(running));
        assert_eq!(digest(&runtime.runtime_resources(), &healthy_source), healthy_update);
        assert_eq!(digest(&runtime.runtime_resources(), &incomplete_source), incomplete_update);
        assert_eq!(digest(&frozen, &healthy_source), initial_healthy);
        assert_eq!(digest(&frozen, &incomplete_source), initial_incomplete);
        fixture.gates[1].release();
        settlement.wait().await;
        assert_eq!(fixture.provider.request_bodies().len(), 2);
        assert_eq!(fixture.host.drain().await, [] as [String; 0]);
        assert!(client.wait().await.unwrap().success());
        let _ = proxy_join.join();
    }))
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn session_input_commit_fences_a_parked_python_fallback() {
    use super::super::protocol::{call, initialize};
    use crate::app_server::{
        connection::AppServerConnection,
        protocol::{Method, MethodResult},
    };
    use crate::capabilities::ToolSourceId;
    use crate::local_runtime::session_inputs::{SessionConfigContent, SessionInputChange};

    Box::pin(bounded(async {
        let fixture = Fixture::with_session_count(None, 1).await;
        let (_, incomplete) = python_fallback_packages(&fixture.workspaces[0]);
        let mut client = fixture.start_execution(0).await;
        fixture.load(0).await.unwrap().unwrap();
        let id = fixture.sessions[0].id.clone();
        let endpoint = fixture.manager.execution_endpoint(&id).unwrap();
        tokio::task::spawn_blocking({
            let manager = fixture.manager.clone();
            let id = id.clone();
            let endpoint = endpoint.clone();
            move || manager.observe_bound_source(&id, &endpoint, endpoint.control().unwrap(), true)
        })
        .await
        .unwrap();
        applied(&fixture, 0).await;

        std::fs::write(incomplete.join("d000/helper.py"), "HELPER = 2\n").unwrap();
        let (captured, waiting) = tokio::sync::oneshot::channel();
        let (release, parked) = std::sync::mpsc::channel();
        fixture.manager.configuration.test_hooks.insert(
            "before_bound_python_fallback_read",
            move || {
                captured.send(()).unwrap();
                parked.recv().unwrap();
            },
        );
        let old = tokio::task::spawn_blocking({
            let manager = fixture.manager.clone();
            let id = id.clone();
            move || {
                manager.observe_incomplete_bound_python(
                    &id,
                    &endpoint,
                    endpoint.control().unwrap(),
                    &std::collections::BTreeSet::from([ToolSourceId::ManagedPython(
                        "incomplete-pkg".into(),
                    )]),
                )
            }
        });
        waiting.await.unwrap();

        let connection = AppServerConnection::new(fixture.host.clone());
        initialize(&connection).await;
        let (revision, _) = fixture.manager.sessions.read_settings(&id).await.unwrap();
        let session_config =
            SessionConfigContent("[tool_deadline_policy]\nhard_deadline_ms=12345\n".into());
        let MethodResult::SessionInputsWritten { revision: written } = call(
            &connection,
            1,
            Method::SessionInputsWrite {
                session_id: id.clone(),
                expected_revision: revision,
                session_config: Some(SessionInputChange::Replace(session_config.clone())),
                session_resources: None,
            },
        )
        .await
        else {
            panic!("committed Session configuration");
        };
        assert_eq!(written, revision + 1);
        let adopted = applied(&fixture, 0).await.desired;
        release.send(()).unwrap();
        assert!(!old.await.unwrap(), "the obsolete fallback did not publish");
        let (persisted_revision, persisted) =
            fixture.manager.sessions.read_settings(&id).await.unwrap();
        assert_eq!(persisted_revision, written);
        assert_eq!(persisted.session_config, Some(session_config));
        assert_eq!(
            fixture
                .manager
                .configuration_application(&id)
                .unwrap()
                .desired,
            adopted
        );
        {
            let application = fixture.manager.applications.lock();
            let (source, _) = application.latest_bound_source(id.as_str()).unwrap();
            assert_eq!(source.input.authoring, persisted.input().authoring);
        }
        assert_eq!(fixture.provider.request_bodies().len(), 0);
        assert_eq!(fixture.host.drain().await, [] as [String; 0]);
        assert!(client.wait().await.unwrap().success());
    }))
    .await;
}
