//! Session creation bindings: exact native application identity, the created
//! Session's model catalog, initial model commits and multi-client visibility.
#![allow(clippy::large_futures)] // bounded fixture futures; no recursive or unbounded stack growth
use super::*;

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
        assert!(diagnostic.contains("local/missing"), "{diagnostic}");
        assert_eq!(count().await, before);
        assert!(create(broken).await.is_err());
        fixture.close().await;
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
                    cwd: f.workspaces[0].clone(),
                    model: explicit.then(|| selection.clone()),
                })
                .await
                .unwrap();
            assert!(created.durability_diagnostic.is_none());
            assert!(f.manager.registry.0.lock().unwrap().entries.is_empty());
            assert!(f.provider.request_bodies().is_empty());
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
            cwd: f.workspaces[0].clone(),
            model: Some(crate::model::session::SessionModelConfig::of(
                crate::model::catalog::ModelRef::parse("local/missing").unwrap(),
            )),
        })
        .await;
    assert!(result.is_err());
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
    assert!(f.provider.request_bodies().is_empty());
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
                        submission: submitted.then(|| {
                            crate::local_runtime::mcp_drafts::McpDraftTarget {
                                draft_id: "visibility-regression".into(),
                                generation: 1,
                                host_authority: "fixture-host".into(),
                            }
                        }),
                        settings: SessionPersistentState { cwd, model: None },
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
            let instructions_a = document["agent"].get("instructions").cloned();
            document["agent"]["model"]["model"] = "local/b".into();
            document["agent"]
                .as_table_mut()
                .unwrap()
                .insert("instructions".into(), "configuration B".into());
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
                        cwd: f.workspaces[0].clone(),
                        model: None,
                    },
                    &f.manager.credentials,
                    false,
                )
                .unwrap();
            assert_eq!(fresh.session_model().model.to_string(), "local/b");
            assert_eq!(fresh.config().agent.instructions, "configuration B");
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
            assert_ne!(retained.config().agent.instructions, "configuration B");
            if let Some(toml::Value::String(text)) = instructions_a {
                assert_eq!(retained.config().agent.instructions, text);
            }
            let MethodResult::Attached { target, .. } = super::super::protocol::call(
                &observer,
                3,
                Method::SessionAttach {
                    session_id: id.clone(),
                    node_id: None,
                },
            )
            .await
            else {
                panic!()
            };
            let runtime = f.manager.configuration_runtime(&id).unwrap();
            assert_eq!(
                runtime
                    .runtime_resources()
                    .configuration()
                    .unwrap()
                    .config
                    .agent
                    .instructions,
                retained.config().agent.instructions
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
                runtime
                    .runtime_resources()
                    .configuration()
                    .unwrap()
                    .config
                    .agent
                    .instructions,
                retained.config().agent.instructions
            );
            publication.before_catalog_publication.release();
            let application = session_settled(&f, &id).await;
            assert!(matches!(
                application.units[&ApplyUnit::Instructions],
                UnitApplication::Ready { .. }
            ));
            let published = f.manager.sessions.configuration_bindings.lock().unwrap()[&id].clone();
            assert_eq!(published.binding_revision, retained.binding_revision + 1);
            assert_eq!(
                runtime.configuration_cut().binding_revision,
                published.binding_revision
            );
            assert_eq!(runtime.model_view().configured, *retained.session_model());
            assert_eq!(
                runtime
                    .runtime_resources()
                    .configuration()
                    .unwrap()
                    .config
                    .agent
                    .instructions,
                retained.config().agent.instructions
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
    assert!(
        f.manager
            .sessions
            .list_sessions(None, 0, 32)
            .await
            .unwrap()
            .sessions
            .is_empty()
    );
    assert_eq!(log.next_after(frontier), None);
    f.close().await;
}
