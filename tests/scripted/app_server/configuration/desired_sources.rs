//! Desired-source generations of sessionless Workspaces after User commits,
//! keeping the last good available binding when preparation fails.
#![allow(clippy::large_futures)] // bounded fixture futures; no recursive or unbounded stack growth
use super::*;

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
