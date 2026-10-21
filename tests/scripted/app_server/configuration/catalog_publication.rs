//! Model catalog publication on one authority: discovery and admission agree,
//! unavailable selections survive reloads, selections race publication and
//! recovery keeps explicit policy explicit.
#![allow(clippy::large_futures)] // bounded fixture futures; no recursive or unbounded stack growth
use super::*;

/// Issue #459: one published catalog is the authority for a Session's
/// discovery and selection admission, through the App Server routing
/// (`SessionRuntimeManager::set_model`) and the Runtime Client alike. Edits,
/// removals and default moves of unselected Profiles publish without
/// preparing or rebuilding anything; a failed or overtaken publication never
/// replaces the last good authority.
#[allow(clippy::too_many_lines)] // one complete publication contract
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue459_catalog_publication_keeps_discovery_and_admission_on_one_authority() {
    use crate::local_runtime::configuration::settings::SourceTarget;
    use crate::model::authoring::Profile;
    use crate::model::catalog::{ModelProfileId, ModelRef};
    use crate::model::session::SessionModelConfig;
    use crate::toml_authoring::AuthoredRequestParams;
    use serde_json::json;
    type Authored<'a> = (&'a str, Option<u32>, serde_json::Value);
    Box::pin(bounded(async {
        let fixture = Fixture::with_session_count(None, 1).await;
        let id = fixture.sessions[0].id.clone();
        fixture.manager.load(&id, None).await.unwrap();
        let runtime = fixture.manager.configuration_runtime(&id).unwrap();
        let probe = fixture
            .manager
            .probe(&fixture.sessions[0].active_conversation_id);
        let host = fixture.manager.configuration_host(&id).unwrap();
        let (client, _) = host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
            .unwrap();
        let authored = fixture
            .manager
            .source_settings(&SourceTarget::User)
            .await
            .unwrap()
            .user
            .authored
            .unwrap()
            .models
            .unwrap()["local/a"]
            .clone();
        let model = |default: &str, profiles: &[Authored]| {
            let mut model = authored.clone();
            model.request_params = None;
            model.default_profile = Some(ModelProfileId::new(default));
            model.profiles = Some(
                profiles
                    .iter()
                    .map(|(name, budget, value)| {
                        (
                            ModelProfileId::new(*name),
                            Profile {
                                reasoning_enabled: None,
                                max_output_tokens: *budget,
                                request_params: AuthoredRequestParams(
                                    serde_json::from_value(value.clone()).unwrap(),
                                ),
                            },
                        )
                    })
                    .collect(),
            );
            ConfigMutation::Model {
                id: "local/a".into(),
                authored: Some(model),
            }
        };
        let pinned = |profile: &str| SessionModelConfig {
            profile: Some(ModelProfileId::new(profile)),
            ..SessionModelConfig::of(ModelRef::parse("local/a").unwrap())
        };
        let entry = |catalog: &super::super::protocol::ModelCatalogObservation| {
            let model = catalog
                .models
                .iter()
                .find(|model| model.model.to_string() == "local/a")
                .unwrap()
                .clone();
            let profiles: Vec<_> = model
                .profiles
                .iter()
                .map(|profile| (profile.id.to_string(), profile.max_output_tokens))
                .collect();
            (model.default_profile.map(|id| id.to_string()), profiles)
        };
        // Discovery through the Runtime Client equals the App Server's read.
        let discovered = || {
            let catalog =
                super::super::protocol::read_native_catalog_result(client.model_catalog().unwrap());
            assert_eq!(catalog, runtime.model_catalog());
            entry(&catalog)
        };
        let balanced: Authored = ("balanced", None, json!({"temperature": 0.5}));
        let fast: Authored = ("fast", Some(64), json!({"temperature": 1.0}));
        let creative: Authored = ("creative", None, json!({"temperature": 1.2}));
        write(
            &fixture,
            0,
            model(
                "balanced",
                &[balanced.clone(), fast.clone(), creative.clone()],
            ),
        )
        .await;
        let candidate = settled(&fixture, 0)
            .await
            .candidate
            .expect("the selected Model gained Profiles");
        fixture
            .manager
            .adopt_configuration(&id, &candidate.identity, candidate.expected_binding)
            .unwrap();
        fixture
            .manager
            .set_model(&id, pinned("balanced"))
            .await
            .unwrap();
        settled(&fixture, 0).await;
        let invocation = runtime.model_snapshot();
        let capability = runtime.runtime_resources().capability().clone();
        let preparations = probe.configuration_preparations.load(Ordering::SeqCst);
        let published = |expected: (Option<&str>, &[(&str, Option<u32>)])| {
            let expected = (
                expected.0.map(str::to_owned),
                expected
                    .1
                    .iter()
                    .map(|(id, budget)| ((*id).to_owned(), *budget))
                    .collect::<Vec<_>>(),
            );
            assert_eq!(discovered(), expected);
            // Publication alone never re-resolves, prepares or rebuilds.
            assert_eq!(runtime.model_snapshot(), invocation);
            assert!(Arc::ptr_eq(
                &capability,
                runtime.runtime_resources().capability()
            ));
            assert_eq!(
                probe.configuration_preparations.load(Ordering::SeqCst),
                preparations
            );
        };
        let applied = |application: ConfigurationApplication| {
            assert!(application.candidate.is_none(), "{application:?}");
            assert!(
                application
                    .units
                    .values()
                    .all(|unit| *unit == UnitApplication::Applied),
                "{application:?}"
            );
        };

        // An edited unselected Profile is published.
        let fast_edited: Authored = ("fast", Some(32), json!({"temperature": 1.5, "top_p": 0.9}));
        write(
            &fixture,
            0,
            model(
                "balanced",
                &[balanced.clone(), fast_edited.clone(), creative.clone()],
            ),
        )
        .await;
        applied(settled(&fixture, 0).await);
        published((
            Some("balanced"),
            &[("balanced", None), ("creative", None), ("fast", Some(32))],
        ));

        // A removed unselected Profile is no longer selectable by its Session owner.
        write(
            &fixture,
            0,
            model("balanced", &[balanced.clone(), fast_edited.clone()]),
        )
        .await;
        applied(settled(&fixture, 0).await);
        published((Some("balanced"), &[("balanced", None), ("fast", Some(32))]));
        let view = runtime.model_view();
        assert!(matches!(
            fixture.manager.set_model(&id, pinned("creative")).await,
            Err(ModelSelectionError::Model(
                crate::runtime::conversation_runtime::ModelUpdateError::InvalidConfiguration(..)
            ))
        ));
        let observed_model =
            super::super::protocol::read_native_model_result(&client, client.model_get().unwrap())
                .await;
        assert_eq!(observed_model, view);
        assert_eq!(runtime.model_view(), view);

        // A moved default never reaches the pinned selection.
        write(
            &fixture,
            0,
            model("fast", &[balanced.clone(), fast_edited.clone()]),
        )
        .await;
        applied(settled(&fixture, 0).await);
        published((Some("fast"), &[("balanced", None), ("fast", Some(32))]));

        // A failed publication keeps the last good authority.
        let revision = runtime.runtime_resources().revision();
        let path = fixture
            .manager
            .source_settings(&SourceTarget::User)
            .await
            .unwrap()
            .user
            .path;
        let good = std::fs::read_to_string(&path).unwrap();
        let mut document: toml::Value = toml::from_str(&good).unwrap();
        document["models"]["local/a"]["profiles"]
            .as_table_mut()
            .unwrap()
            .insert(
                "deep".into(),
                toml::Value::try_from(json!({"request_params": "{\"temperature\":0.1}"})).unwrap(),
            );
        document["agent"]["model"]["model"] = "local/missing".into();
        std::fs::write(&path, toml::to_string(&document).unwrap()).unwrap();
        let workspace = SourceTarget::Workspace {
            directory: fixture.workspaces[0].clone(),
        };
        fixture
            .manager
            .reconcile_configuration(&workspace)
            .await
            .unwrap();
        let failed = settled(&fixture, 0).await;
        assert!(
            failed
                .units
                .values()
                .any(|unit| matches!(unit, UnitApplication::Failed { .. })),
            "{failed:?}"
        );
        published((Some("fast"), &[("balanced", None), ("fast", Some(32))]));
        assert_eq!(runtime.runtime_resources().revision(), revision);
        assert!(
            fixture
                .manager
                .set_model(&id, pinned("deep"))
                .await
                .is_err()
        );
        std::fs::write(&path, good).unwrap();
        fixture
            .manager
            .reconcile_configuration(&workspace)
            .await
            .unwrap();
        applied(settled(&fixture, 0).await);
        assert_eq!(runtime.runtime_resources().revision(), revision);

        // An overtaken publication never becomes visible: only the newer
        // generation publishes, exactly once.
        probe.before_catalog_publication.arm();
        let deep: Authored = ("deep", None, json!({"temperature": 0.1}));
        write(
            &fixture,
            0,
            model("fast", &[balanced.clone(), fast_edited.clone(), deep]),
        )
        .await;
        probe.before_catalog_publication.entered().await;
        let deeper: Authored = ("deeper", None, json!({"temperature": 0.05}));
        write(
            &fixture,
            0,
            model("fast", &[balanced.clone(), fast_edited.clone(), deeper]),
        )
        .await;
        probe.before_catalog_publication.release();
        applied(settled(&fixture, 0).await);
        published((
            Some("fast"),
            &[("balanced", None), ("deeper", None), ("fast", Some(32))],
        ));
        assert_eq!(runtime.runtime_resources().revision(), revision.next());

        // Selection admission resolves the edited Profile from the published
        // catalog, never a retained older registry.
        let model = fixture
            .manager
            .set_model(&id, pinned("fast"))
            .await
            .unwrap()
            .0;
        assert_eq!(
            model.effective.as_ref().unwrap().request_params,
            serde_json::from_value::<crate::model::invocation::RequestParams>(
                json!({"temperature": 1.5, "top_p": 0.9})
            )
            .unwrap()
        );
        assert_eq!(model.effective.as_ref().unwrap().max_output_tokens, 32);
        drop(client);
        fixture.close().await;
    }))
    .await;
}

/// Issue #459: an unavailable selection is a Session fact, not a load
/// failure. After residency loss the Session composes from its retained
/// binding — the published catalog — as unavailable, with nothing resolved
/// for the removed selection; after losing the retained binding too (as
/// across a restart) the persisted selection binds the same way. Both refuse
/// work and both recover through `session/setModel`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue459_unavailable_selection_survives_reload_and_cold_binding() {
    use crate::model::catalog::ModelRef;
    use crate::model::session::SessionModelConfig;
    use crate::runtime::conversation_runtime::InboundAdmissionError;
    Box::pin(bounded(async {
        let fixture = Fixture::with_session_count(None, 1).await;
        let id = fixture.sessions[0].id.clone();
        let conversation = fixture.sessions[0].active_conversation_id.clone();
        fixture.manager.load(&id, None).await.unwrap();
        let b = SessionModelConfig::of(ModelRef::parse("local/b").unwrap());
        fixture.manager.set_model(&id, b.clone()).await.unwrap();
        assert!(fixture.manager.configuration_application(&id).is_none());
        write(
            &fixture,
            0,
            ConfigMutation::Model {
                id: "local/b".into(),
                authored: None,
            },
        )
        .await;
        let application = settled(&fixture, 0).await;
        assert!(application.candidate.is_none(), "{application:?}");
        let resident = fixture
            .manager
            .configuration_runtime(&id)
            .unwrap()
            .model_view();
        assert_eq!(resident.configured, b);
        assert!(resident.unavailable.is_some());
        assert!(
            resident.effective.is_some(),
            "the last resolved invocation is retained"
        );

        let reload = || async {
            fixture.manager.unload(&conversation).await.unwrap();
            fixture.manager.load(&id, None).await.unwrap();
            let runtime = fixture.manager.configuration_runtime(&id).unwrap();
            let view = runtime.model_view();
            assert_eq!(view.configured, b);
            assert!(view.unavailable.as_deref().unwrap().contains("local/b"));
            // Nothing is resolved for, or substituted into, a removed selection.
            assert!(view.effective.is_none() && view.summary.is_none());
            assert!(
                runtime
                    .model_catalog()
                    .models
                    .iter()
                    .all(|model| model.model.to_string() != "local/b")
            );
            assert!(matches!(
                runtime.submit_inbound(issue459_text("refused")),
                Err(InboundAdmissionError::ModelUnavailable { .. })
            ));
            runtime
        };
        // Residency loss: composition from the retained, published binding.
        reload().await;
        // Restart-equivalent: no retained binding, only persisted settings.
        fixture.manager.unload(&conversation).await.unwrap();
        fixture
            .manager
            .sessions
            .configuration_bindings
            .lock()
            .unwrap()
            .remove(&id);
        fixture.manager.load(&id, None).await.unwrap();
        let runtime = reload().await;
        let view = fixture
            .manager
            .set_model(&id, issue459_pinned(None))
            .await
            .unwrap()
            .0;
        assert!(view.unavailable.is_none());
        assert_eq!(runtime.model_view(), view);
        assert!(fixture.provider.request_bodies().is_empty());
        fixture.close().await;
    }))
    .await;
}

/// Issue #459: recovery leaves ordinary adoption intact. A selected Profile
/// whose parameters change (and stays valid) is still prepared as a
/// candidate; the live invocation is not replaced, the admitted Attempt keeps
/// its frozen parameters, and a selection committed after preparation
/// overtakes the candidate's expected binding so it can no longer adopt.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue459_changed_selected_profile_still_needs_fenced_adoption() {
    use serde_json::json;
    Box::pin(bounded(async {
        let fixture = Fixture::with_session_count(None, 1).await;
        let id = fixture.sessions[0].id.clone();
        fixture.manager.load(&id, None).await.unwrap();
        let runtime = fixture.manager.configuration_runtime(&id).unwrap();
        let host = fixture.manager.configuration_host(&id).unwrap();
        let (client, _) = host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
            .unwrap();
        let authored = issue459_authored_a(&fixture).await;
        let fast = ("fast", json!({"temperature": 1.0}));
        write(
            &fixture,
            0,
            issue459_profiles(
                &authored,
                "balanced",
                &[("balanced", json!({"temperature": 0.5})), fast.clone()],
            ),
        )
        .await;
        let candidate = settled(&fixture, 0).await.candidate.unwrap();
        fixture
            .manager
            .adopt_configuration(&id, &candidate.identity, candidate.expected_binding)
            .unwrap();
        fixture
            .manager
            .set_model(&id, issue459_pinned(Some("balanced")))
            .await
            .unwrap();
        settled(&fixture, 0).await;
        let invocation = runtime.model_snapshot();
        let mut settlement = SettlementReceiptWait::capture(&runtime);
        runtime.submit_inbound(issue459_text("request-A")).unwrap();
        fixture.gates[0].wait_entered().await;
        let admitted = host
            .checkpoint()
            .map(|checkpoint| (checkpoint.snapshot, checkpoint.cursor))
            .unwrap()
            .0
            .attempt
            .unwrap()
            .model;

        // The selected Profile's parameters change; it stays valid.
        write(
            &fixture,
            0,
            issue459_profiles(
                &authored,
                "balanced",
                &[("balanced", json!({"temperature": 0.7})), fast.clone()],
            ),
        )
        .await;
        let application = settled(&fixture, 0).await;
        let candidate = application
            .candidate
            .clone()
            .expect("a changed invocation is prepared");
        assert!(
            matches!(
                application.units[&ApplyUnit::Provider],
                UnitApplication::Ready { .. }
            ),
            "{application:?}"
        );
        assert_eq!(runtime.model_snapshot(), invocation);
        assert!(runtime.model_view().unavailable.is_none());
        assert_eq!(
            host.checkpoint()
                .map(|checkpoint| (checkpoint.snapshot, checkpoint.cursor))
                .unwrap()
                .0
                .attempt
                .unwrap()
                .model,
            admitted
        );
        fixture.gates[0].release();
        settlement.wait().await;
        let body: serde_json::Value =
            serde_json::from_str(&fixture.provider.request_bodies()[0]).unwrap();
        assert_eq!(body["temperature"], json!(0.5));

        // A selection committed after preparation advances the binding the
        // candidate expected: it cannot overwrite that selection.
        let model = fixture
            .manager
            .set_model(&id, issue459_pinned(Some("fast")))
            .await
            .unwrap()
            .0;
        assert!(matches!(
            fixture.manager.adopt_configuration(
                &id,
                &candidate.identity,
                candidate.expected_binding
            ),
            Err(AdoptionError::Conflict)
        ));
        assert_eq!(runtime.model_view(), model);
        assert_eq!(
            model.effective.unwrap().request_params["temperature"],
            json!(1.0)
        );
        drop(client);
        fixture.close().await;
    }))
    .await;
}

/// Issue #459: a selection change racing a catalog publication linearizes on
/// one authority. While a publication that removes `fast` is parked before
/// its fence, `session/setModel` admits `fast` against the catalog still
/// published and overtakes the parked generation; the recaptured generation
/// then publishes once, after the selection, and reports it unavailable —
/// never replaced. Discovery and admission agree on that one catalog.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue459_selection_racing_catalog_publication_linearizes_on_one_authority() {
    use serde_json::json;
    Box::pin(bounded(async {
        let fixture = Fixture::with_session_count(None, 1).await;
        let id = fixture.sessions[0].id.clone();
        fixture.manager.load(&id, None).await.unwrap();
        let runtime = fixture.manager.configuration_runtime(&id).unwrap();
        let probe = fixture
            .manager
            .probe(&fixture.sessions[0].active_conversation_id);
        let host = fixture.manager.configuration_host(&id).unwrap();
        let (client, _) = host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
            .unwrap();
        let authored = issue459_authored_a(&fixture).await;
        let balanced = ("balanced", json!({"temperature": 0.5}));
        let fast = ("fast", json!({"temperature": 1.0}));
        let deep = ("deep", json!({"temperature": 0.1}));
        write(
            &fixture,
            0,
            issue459_profiles(
                &authored,
                "balanced",
                &[balanced.clone(), fast.clone(), deep.clone()],
            ),
        )
        .await;
        let candidate = settled(&fixture, 0).await.candidate.unwrap();
        fixture
            .manager
            .adopt_configuration(&id, &candidate.identity, candidate.expected_binding)
            .unwrap();
        fixture
            .manager
            .set_model(&id, issue459_pinned(Some("balanced")))
            .await
            .unwrap();
        settled(&fixture, 0).await;
        let revision = runtime.runtime_resources().revision();

        probe.before_catalog_publication.arm();
        write(
            &fixture,
            0,
            issue459_profiles(&authored, "balanced", &[balanced.clone(), deep.clone()]),
        )
        .await;
        probe.before_catalog_publication.entered().await;
        // Linearizes first, against the catalog that is still published.
        let selected = fixture
            .manager
            .set_model(&id, issue459_pinned(Some("fast")))
            .await
            .unwrap()
            .0;
        assert!(selected.unavailable.is_none());
        assert_eq!(
            selected.effective.as_ref().unwrap().request_params["temperature"],
            json!(1.0)
        );
        probe.before_catalog_publication.release();
        let application = settled(&fixture, 0).await;
        assert!(application.candidate.is_none(), "{application:?}");
        assert!(
            application
                .units
                .values()
                .all(|unit| *unit == UnitApplication::Applied),
            "{application:?}"
        );

        // Published exactly once, after the selection, which it reports
        // unavailable while keeping it configured.
        assert_eq!(runtime.runtime_resources().revision(), revision.next());
        let view = runtime.model_view();
        assert_eq!(view.configured, issue459_pinned(Some("fast")));
        assert!(view.unavailable.as_deref().unwrap().contains("fast"));
        assert_eq!(view.effective, selected.effective);
        let catalog =
            super::super::protocol::read_native_catalog_result(client.model_catalog().unwrap());
        assert_eq!(catalog, runtime.model_catalog());
        let profiles: Vec<_> = catalog
            .models
            .iter()
            .find(|model| model.model.to_string() == "local/a")
            .unwrap()
            .profiles
            .iter()
            .map(|profile| profile.id.to_string())
            .collect();
        assert_eq!(profiles, ["balanced", "deep"]);
        assert!(matches!(
            fixture
                .manager
                .set_model(&id, issue459_pinned(Some("fast")))
                .await,
            Err(ModelSelectionError::Model(
                crate::runtime::conversation_runtime::ModelUpdateError::InvalidConfiguration(..)
            ))
        ));
        let model = fixture
            .manager
            .set_model(&id, issue459_pinned(Some("deep")))
            .await
            .unwrap()
            .0;
        assert!(model.unavailable.is_none());
        assert_eq!(
            model.effective.unwrap().request_params["temperature"],
            json!(0.1)
        );
        drop(client);
        fixture.close().await;
    }))
    .await;
}

/// Issue #459: recovery keeps the explicit Summary policy the Session's own
/// choice. A removed primary Profile is replaced while the valid explicit
/// Summary is kept; a removed Summary Model makes the selection unavailable
/// on its own; switching only the primary while keeping that invalid Summary
/// is refused explicitly; a complete valid configuration succeeds. Nothing
/// resets the Summary automatically.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue459_recovery_keeps_the_explicit_summary_policy_explicit() {
    use crate::model::catalog::ModelRef;
    use crate::model::invocation::RequestParams;
    use crate::model::session::{SessionModelConfig, SummaryModelPolicy, SummaryModelView};
    use crate::runtime::conversation_runtime::InboundAdmissionError;
    use serde_json::json;
    Box::pin(bounded(async {
        let fixture = Fixture::with_session_count(None, 1).await;
        let id = fixture.sessions[0].id.clone();
        fixture.manager.load(&id, None).await.unwrap();
        let runtime = fixture.manager.configuration_runtime(&id).unwrap();
        let authored = issue459_authored_a(&fixture).await;
        let fast = ("fast", json!({"temperature": 1.0}));
        let deep = ("deep", json!({"temperature": 0.1}));
        write(
            &fixture,
            0,
            issue459_profiles(&authored, "deep", &[fast.clone(), deep.clone()]),
        )
        .await;
        let candidate = settled(&fixture, 0).await.candidate.unwrap();
        fixture
            .manager
            .adopt_configuration(&id, &candidate.identity, candidate.expected_binding)
            .unwrap();
        let summary_on = |model: &str| SummaryModelPolicy::Explicit {
            model: ModelRef::parse(model).unwrap(),
            profile: None,
            request_params: RequestParams::new(),
            max_output_tokens: Some(256),
        };
        let with_summary = |profile, summary| SessionModelConfig {
            summary_model: summary,
            ..issue459_pinned(profile)
        };
        fixture
            .manager
            .set_model(&id, with_summary(Some("fast"), summary_on("local/b")))
            .await
            .unwrap();
        settled(&fixture, 0).await;

        // The primary Profile is removed; the explicit Summary stays valid.
        write(
            &fixture,
            0,
            issue459_profiles(&authored, "deep", std::slice::from_ref(&deep)),
        )
        .await;
        settled(&fixture, 0).await;
        let view = runtime.model_view();
        assert!(view.unavailable.as_deref().unwrap().contains("fast"));
        assert_eq!(view.configured.summary_model, summary_on("local/b"));
        // Replacing the primary keeps the explicit Summary, as both clients send it.
        let view = fixture
            .manager
            .set_model(&id, with_summary(Some("deep"), summary_on("local/b")))
            .await
            .unwrap()
            .0;
        assert!(view.unavailable.is_none());
        let Some(SummaryModelView::Explicit(summary)) = &view.summary else {
            panic!("explicit summary")
        };
        assert_eq!(
            (summary.model.to_string(), summary.max_output_tokens),
            ("local/b".to_owned(), 256)
        );
        settled(&fixture, 0).await;

        // The explicit Summary's Model is removed; the primary stays valid.
        write(
            &fixture,
            0,
            ConfigMutation::Model {
                id: "local/b".into(),
                authored: None,
            },
        )
        .await;
        settled(&fixture, 0).await;
        let view = runtime.model_view();
        assert!(view.unavailable.as_deref().unwrap().contains("local/b"));
        assert_eq!(
            view.configured,
            with_summary(Some("deep"), summary_on("local/b"))
        );
        assert!(matches!(
            runtime.submit_inbound(issue459_text("refused")),
            Err(InboundAdmissionError::ModelUnavailable { .. })
        ));
        // Switching only the primary keeps the invalid Summary and is refused.
        let Err(ModelSelectionError::Model(
            crate::runtime::conversation_runtime::ModelUpdateError::InvalidConfiguration(
                diagnostic,
            ),
        )) = fixture
            .manager
            .set_model(&id, with_summary(None, summary_on("local/b")))
            .await
        else {
            panic!("an invalid explicit Summary is refused")
        };
        assert!(diagnostic.contains("local/b"), "{diagnostic}");
        assert_eq!(runtime.model_view(), view);
        // A complete valid configuration corrects both.
        let view = fixture
            .manager
            .set_model(&id, with_summary(None, summary_on("local/a")))
            .await
            .unwrap()
            .0;
        assert!(view.unavailable.is_none());
        assert_eq!(view.configured.summary_model, summary_on("local/a"));
        assert!(fixture.provider.request_bodies().is_empty());
        fixture.close().await;
    }))
    .await;
}

/// Issue #459: catalog publication metadata says what was published and what
/// was not. One User source update edits an unselected Model and the process
/// shutdown deadline, which needs a restart. The catalog publishes as the
/// whole Provider unit — its values, provenance and component revision —
/// while the adopted-source baseline (`source_revisions`), the other units'
/// revisions and the unapplied process field stay as they were. Reconciling
/// the same sources again publishes nothing; adopting a real context change
/// then moves the baseline and the units it composes forward.
#[allow(clippy::too_many_lines)] // one metadata contract across three generations
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue459_catalog_publication_metadata_names_what_was_published() {
    use crate::local_runtime::configuration::settings::SourceTarget;
    Box::pin(bounded(async {
        let fixture = Fixture::with_session_count(None, 1).await;
        let id = fixture.sessions[0].id.clone();
        fixture.manager.load(&id, None).await.unwrap();
        let runtime = fixture.manager.configuration_runtime(&id).unwrap();
        let retained = || {
            fixture
                .manager
                .sessions
                .configuration_bindings
                .lock()
                .unwrap()[&id]
                .clone()
        };
        let before = runtime.runtime_resources();
        let old = before.configuration().unwrap().clone();
        let invocation = runtime.model_snapshot();
        let process = fixture.manager.process_policy();

        let path = fixture
            .manager
            .source_settings(&SourceTarget::User)
            .await
            .unwrap()
            .user
            .path;
        let mut document: toml::Value =
            toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        document["models"]["local/b"]
            .as_table_mut()
            .unwrap()
            .insert("max_output_tokens".into(), 1024.into());
        document.as_table_mut().unwrap().insert(
            "app_server".into(),
            toml::toml! { shutdown_deadline_ms = 12_345 }.into(),
        );
        std::fs::write(&path, toml::to_string(&document).unwrap()).unwrap();
        let workspace = SourceTarget::Workspace {
            directory: fixture.workspaces[0].clone(),
        };
        fixture
            .manager
            .reconcile_configuration(&workspace)
            .await
            .unwrap();
        let application = settled(&fixture, 0).await;
        let published_revision = application.desired.input_revision.clone().unwrap();
        assert!(application.candidate.is_none(), "{application:?}");
        assert_eq!(
            application.units[&ApplyUnit::ProcessBindings],
            UnitApplication::ProcessRestart
        );
        for unit in [
            ApplyUnit::Capabilities,
            ApplyUnit::Instructions,
            ApplyUnit::Provider,
        ] {
            assert_eq!(
                application.units[&unit],
                UnitApplication::Applied,
                "{application:?}"
            );
        }

        // Discovery reflects the catalog; the invocation and capabilities do not move.
        let entry = runtime
            .model_catalog()
            .models
            .into_iter()
            .find(|model| model.model.to_string() == "local/b")
            .unwrap();
        assert_eq!(entry.max_output_tokens, 1024);
        assert_eq!(runtime.model_snapshot(), invocation);
        let after = runtime.runtime_resources();
        assert!(Arc::ptr_eq(before.capability(), after.capability()));
        assert_eq!(after.revision(), before.revision().next());
        let new = after.configuration().unwrap().clone();
        // The Provider unit is published with its own component revision; the
        // adopted-source baseline and every other unit keep theirs.
        assert_eq!(
            new.authored.component_revisions[&ApplyUnit::Provider],
            published_revision
        );
        assert_ne!(
            old.authored.component_revisions[&ApplyUnit::Provider],
            published_revision
        );
        for unit in [
            ApplyUnit::Capabilities,
            ApplyUnit::Instructions,
            ApplyUnit::ExecutionPolicy,
            ApplyUnit::SharedCapacity,
        ] {
            assert_eq!(
                new.authored.component_revisions[&unit], old.authored.component_revisions[&unit],
                "{unit:?}"
            );
        }
        assert_eq!(new.authored.source_revisions, old.authored.source_revisions);
        assert_eq!(
            new.authored.effective.models.as_ref().unwrap()["local/b"].max_output_tokens,
            1024
        );
        // The restart-bound process field is neither in the generation nor live.
        assert_eq!(
            new.authored.effective.app_server,
            old.authored.effective.app_server
        );
        assert_eq!(
            fixture.manager.process_policy().shutdown_deadline_ms,
            process.shutdown_deadline_ms
        );
        // The retained binding carries the same facts, so a reload composes
        // exactly this generation's metadata.
        let binding = retained();
        assert_eq!(
            binding.component_revisions,
            new.authored.component_revisions
        );
        assert_eq!(binding.source_revisions, old.authored.source_revisions);
        // The wire projection keeps the adopted-source baseline.
        let view = crate::local_runtime::configuration::settings::EffectiveConfiguration::adopted(
            &runtime,
        )
        .unwrap();
        assert_eq!(view.source_revisions, old.authored.source_revisions);
        assert_eq!(
            view.document.models.as_ref().unwrap()["local/b"].max_output_tokens,
            1024
        );

        // The same sources again publish nothing.
        fixture
            .manager
            .reconcile_configuration(&workspace)
            .await
            .unwrap();
        let again = settled(&fixture, 0).await;
        assert_eq!(again.desired, application.desired);
        assert!(Arc::ptr_eq(&after, &runtime.runtime_resources()));

        // Adopting a real context change moves the baseline forward.
        write(
            &fixture,
            0,
            ConfigMutation::Instructions {
                authored: Some("issue459 adopted".into()),
            },
        )
        .await;
        let candidate = settled(&fixture, 0)
            .await
            .candidate
            .expect("an instruction change is prepared");
        fixture
            .manager
            .adopt_configuration(&id, &candidate.identity, candidate.expected_binding)
            .unwrap();
        let adopted = runtime.runtime_resources().configuration().unwrap().clone();
        let binding = retained();
        assert_ne!(
            adopted.authored.source_revisions,
            old.authored.source_revisions
        );
        assert_eq!(adopted.authored.source_revisions, binding.source_revisions);
        // A context adoption composes the Instructions and Provider units at
        // its revision; units it did not compose keep theirs.
        let revision = candidate.identity.input_revision.unwrap();
        for unit in [ApplyUnit::Instructions, ApplyUnit::Provider] {
            assert_eq!(
                adopted.authored.component_revisions[&unit], revision,
                "{unit:?}"
            );
        }
        for unit in [
            ApplyUnit::Capabilities,
            ApplyUnit::ExecutionPolicy,
            ApplyUnit::SharedCapacity,
        ] {
            assert_eq!(
                adopted.authored.component_revisions[&unit],
                old.authored.component_revisions[&unit],
                "{unit:?}"
            );
        }
        assert_eq!(
            adopted.authored.effective.models.as_ref().unwrap()["local/b"].max_output_tokens,
            1024
        );
        // Adoption re-resolves through its own registry; the invocation's
        // value is unchanged.
        assert_eq!(runtime.model_snapshot().view(), invocation.view());
        fixture.close().await;
    }))
    .await;
}
