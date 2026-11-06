//! Zero-Session source authority: commits transfer ownership before a lost
//! response, inert isolated sources, hot restart and validated repair, and
//! source authoring disjoint from model selection.
#![allow(clippy::large_futures)] // bounded fixture futures; no recursive or unbounded stack growth
use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn c07_zero_session_commit_transfers_ownership_before_lost_response() {
    use crate::local_runtime::configuration::settings::SourceTarget;
    bounded(async {
        let f = Fixture::with_session_count(None, 0).await;
        let source = f
            .manager
            .source_settings(&SourceTarget::User)
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
                .write_source_settings(
                    &SourceTarget::User,
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
                .source_settings(&SourceTarget::User)
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
            let source = f.manager.source_settings(&target).await.unwrap();
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
                .write_source_settings(
                    &target,
                    revision,
                    SourceMutation::Config {
                        mutation: ConfigMutation::NativeTools {
                            authored: Some(vec![]),
                        },
                    },
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
        {
            let applications = f.manager.applications.lock();
            let mut versions = std::collections::BTreeMap::new();
            while let Some(application) = applications.next_view(&versions) {
                assert!(application.scope.starts_with("source:") && application.units.len() == 1);
                versions.insert(application.scope, application.version);
            }
        }
        let user = f
            .manager
            .source_settings(&SourceTarget::User)
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
            let source = f.manager.source_settings(&target).await.unwrap();
            let mut desired = actual.clone();
            desired.max_connections = 17;
            if restart {
                desired.shutdown_deadline_ms += 1;
            }
            f.manager
                .write_source_settings(
                    &target,
                    source.user.revision,
                    SourceMutation::Config {
                        mutation: ConfigMutation::AppServer {
                            authored: Some(desired),
                        },
                    },
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
            let reopened = f.manager.source_settings(&target).await.unwrap();
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
        let initial = f.manager.source_settings(&target).await.unwrap();
        let bytes = std::fs::read_to_string(&initial.user.path).unwrap();
        std::fs::write(&initial.user.path, "broken = [").unwrap();
        let broken = f.manager.source_settings(&target).await.unwrap();
        assert_ne!(broken.user.revision, initial.user.revision);
        assert!(broken.user.authored.is_none());
        assert!(broken.user.diagnostic.is_some());
        assert!(broken.prospective_resources.is_some());
        assert!(
            f.manager
                .write_source_settings(
                    &target,
                    broken.user.revision.clone(),
                    SourceMutation::RepairConfig {
                        document: "still = [".into()
                    }
                )
                .await
                .is_err()
        );
        let repaired = f
            .manager
            .write_source_settings(
                &target,
                broken.user.revision,
                SourceMutation::RepairConfig { document: bytes },
            )
            .await
            .unwrap();
        let repaired_projection = f.manager.source_settings(&target).await.unwrap();
        assert_eq!(repaired.revision, repaired_projection.user.revision);
        let repaired = repaired_projection;
        assert!(repaired.user.authored.is_some());
        assert!(repaired.user.diagnostic.is_none());
        // Syntactically valid but unresolved sources retain the entire editor and
        // independently valid inventory; a default model is not read admission.
        let mut document: toml::Value =
            toml::from_str(&std::fs::read_to_string(&repaired.user.path).unwrap()).unwrap();
        document["agent"].as_table_mut().unwrap().remove("model");
        std::fs::write(&repaired.user.path, toml::to_string(&document).unwrap()).unwrap();
        let missing = f.manager.source_settings(&target).await.unwrap();
        assert!(missing.user.authored.is_some());
        assert!(missing.prospective_diagnostic.is_some());
        assert!(missing.prospective_resources.is_some());
        document["agent"].as_table_mut().unwrap().insert(
            "model".into(),
            toml::Value::try_from(serde_json::json!({"model":"local/missing"})).unwrap(),
        );
        std::fs::write(&repaired.user.path, toml::to_string(&document).unwrap()).unwrap();
        let unresolved = f.manager.source_settings(&target).await.unwrap();
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
        let original = f.manager.source_settings(&target).await.unwrap();
        for authored in [Some(vec!["read".into()]), Some(vec![]), None] {
            let before = f.manager.source_settings(&target).await.unwrap();
            let after = f
                .manager
                .write_source_settings(
                    &target,
                    before.workspace.unwrap().revision,
                    SourceMutation::Config {
                        mutation: ConfigMutation::NativeTools {
                            authored: authored.clone(),
                        },
                    },
                )
                .await
                .unwrap();
            let after_projection = f.manager.source_settings(&target).await.unwrap();
            assert_eq!(
                after.revision,
                after_projection.workspace.as_ref().unwrap().revision
            );
            let after = after_projection;
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
        let sources = f.manager.source_settings(&target).await.unwrap();
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
        let after = f.manager.source_settings(&target).await.unwrap();
        assert_eq!(after.user, sources.user);
        assert_eq!(after.workspace, sources.workspace);
        f.manager
            .write_source_settings(
                &target,
                after.workspace.unwrap().revision,
                SourceMutation::Config {
                    mutation: ConfigMutation::Instructions {
                        authored: Some("new source guidance".into()),
                    },
                },
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
