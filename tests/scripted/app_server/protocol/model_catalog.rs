//! Published model profiles reaching attached Sessions: publication without
//! adoption, removed selections reported until explicit recovery.
#![allow(clippy::large_futures)] // bounded fixture futures; no recursive or unbounded stack growth
use super::*;

fn profile_ids(catalog: &ModelCatalogObservation) -> Vec<String> {
    catalog
        .models
        .iter()
        .find(|model| model.model.to_string() == "local/a")
        .unwrap()
        .profiles
        .iter()
        .map(|profile| profile.id.to_string())
        .collect()
}

/// Issue #459: catalog publication and invocation adoption are distinct
/// contracts. A Profile published while an attached Session's invocation is
/// unchanged reaches that Session's `session/models` and `session/setModel`
/// without any preparation, adoption candidate or resource rebuild, and the
/// Attempt admitted before it keeps its frozen invocation through a Tool
/// continuation.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue459_published_profile_reaches_attached_session_without_adoption() {
    use crate::local_runtime::configuration::application::UnitApplication;
    use crate::model::catalog::{ModelProfileId, ModelRef};
    use crate::model::session::SessionModelConfig;
    use serde_json::json;
    use std::sync::atomic::Ordering;
    bounded(Box::pin(async {
        let f = Fixture::with_session_count(Some("read"), 1).await;
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        let balanced = ("balanced", json!({"temperature": 0.5}));
        let fast = ("fast", json!({"temperature": 1.0}));
        write_profiles(
            &connection,
            100,
            "balanced",
            &[balanced.clone(), fast.clone()],
        )
        .await;
        let target = attach(&connection, &f, 0).await;
        let pinned = |profile: &str| SessionModelConfig {
            profile: Some(ModelProfileId::new(profile)),
            ..SessionModelConfig::of(ModelRef::parse("local/a").unwrap())
        };
        assert!(matches!(
            call(
                &connection,
                102,
                Method::ModelSet {
                    target: target.clone(),
                    config: Box::new(pinned("balanced")),
                }
            )
            .await,
            MethodResult::ModelSet { .. }
        ));
        let model = read_model_result(
            &connection,
            &target,
            call(
                &connection,
                10_102,
                Method::ModelGet {
                    target: target.clone(),
                },
            )
            .await,
        )
        .await;
        assert_eq!(
            model.effective.as_ref().unwrap().request_params["temperature"],
            json!(0.5)
        );
        // The fixture loads after source authoring; explicitly register its
        // source application. Model selection alone never ingests source files.
        f.manager
            .reconcile_configuration(
                &crate::local_runtime::configuration::settings::SourceTarget::Workspace {
                    directory: f.workspaces[0].clone(),
                },
            )
            .await
            .unwrap();
        super::super::configuration::settled(&f, 0).await;
        let models = |id| {
            let connection = &connection;
            let target = target.clone();
            async move {
                read_catalog_result(call(connection, id, Method::ModelCatalog { target }).await)
            }
        };
        assert_eq!(profile_ids(&models(103).await), ["balanced", "fast"]);

        let managed = f.manager.load(&target.session_id, None).await.unwrap();
        let runtime = f.manager.configuration_runtime(&target.session_id).unwrap();
        let probe = f.manager.probe(&f.sessions[0].active_conversation_id);
        let invocation = runtime.model_snapshot();
        let resources = runtime.runtime_resources();
        let preparations = probe.configuration_preparations.load(Ordering::SeqCst);
        let previous = f
            .manager
            .configuration_application(&target.session_id)
            .unwrap();
        let settlement = runtime.settlement_signal();
        call(
            &connection,
            104,
            Method::TurnStart {
                target: target.clone(),
                content: vec![UserInputBlock::Text(crate::message::content::TextBlock {
                    text: "request-A".into(),
                })],
            },
        )
        .await;
        f.gates[0].wait_entered().await;
        let admitted = managed
            .client()
            .checkpoint()
            .unwrap()
            .snapshot
            .attempt
            .unwrap()
            .model;

        // `deep` is added; `balanced`, the selected Profile, is untouched.
        let deep = ("deep", json!({"temperature": 0.1, "seed": 42}));
        write_profiles(
            &connection,
            105,
            "balanced",
            &[balanced.clone(), fast.clone(), deep.clone()],
        )
        .await;
        let application = super::super::configuration::settled(&f, 0).await;
        assert_ne!(application.desired, previous.desired);
        assert!(application.candidate.is_none(), "{application:?}");
        assert!(
            application
                .units
                .values()
                .all(|unit| *unit == UnitApplication::Applied),
            "{application:?}"
        );
        // Nothing was prepared or rebuilt: the same resolved invocations and
        // adapters, the same capability snapshot; only the configuration
        // generation that carries the catalog advanced.
        assert_eq!(
            probe.configuration_preparations.load(Ordering::SeqCst),
            preparations
        );
        assert_eq!(runtime.model_snapshot(), invocation);
        let published = runtime.runtime_resources();
        assert!(std::sync::Arc::ptr_eq(
            resources.capability(),
            published.capability()
        ));
        assert_eq!(published.revision(), resources.revision().next());
        // Native discovery: the App Server and the Runtime Client read one
        // published catalog.
        let catalog = models(107).await;
        assert_eq!(profile_ids(&catalog), ["balanced", "deep", "fast"]);
        let client = read_native_catalog_result(
            f.manager
                .configuration_host(&target.session_id)
                .unwrap()
                .model_catalog()
                .unwrap(),
        );
        assert_eq!(client, catalog);
        assert_eq!(
            managed
                .client()
                .checkpoint()
                .unwrap()
                .snapshot
                .attempt
                .unwrap()
                .model,
            admitted
        );

        // Select the newly published Profile while the admitted Attempt is still
        // physically parked. Only future independent Attempts may use it.
        let MethodResult::ModelSet { .. } = call(
            &connection,
            108,
            Method::ModelSet {
                target: target.clone(),
                config: Box::new(pinned("deep")),
            },
        )
        .await
        else {
            panic!("bounded selection receipt")
        };
        let model = read_model_result(
            &connection,
            &target,
            call(
                &connection,
                10_108,
                Method::ModelGet {
                    target: target.clone(),
                },
            )
            .await,
        )
        .await;
        assert_eq!(
            model.effective.as_ref().unwrap().profile,
            Some(ModelProfileId::new("deep"))
        );
        assert_eq!(
            managed
                .client()
                .checkpoint()
                .unwrap()
                .snapshot
                .attempt
                .unwrap()
                .model,
            admitted
        );

        // The admitted Attempt's Tool continuation still sends its frozen
        // invocation.
        f.gates[0].release();
        settlement.notified().await;
        let bodies = f.provider.request_bodies();
        assert_eq!(bodies.len(), 2, "Tool proposal and its continuation");
        for body in &bodies {
            let body: serde_json::Value = serde_json::from_str(body).unwrap();
            assert_eq!(body["temperature"], json!(0.5));
            assert!(body.get("seed").is_none());
        }

        let settlement = runtime.settlement_signal();
        call(
            &connection,
            109,
            Method::TurnStart {
                target: target.clone(),
                content: vec![UserInputBlock::Text(crate::message::content::TextBlock {
                    text: "request-A deep".into(),
                })],
            },
        )
        .await;
        settlement.notified().await;
        // The history already holds a Tool result, so the fixture answers
        // this Attempt in one request.
        let bodies = f.provider.request_bodies();
        assert_eq!(bodies.len(), 3);
        let body: serde_json::Value = serde_json::from_str(&bodies[2]).unwrap();
        assert_eq!(
            (&body["temperature"], &body["seed"]),
            (&json!(0.1), &json!(42))
        );
        f.close().await;
    }))
    .await;
}

/// Issue #459: a published catalog that removes the Profile an existing
/// Session pins is published anyway. The Session keeps `fast` configured and
/// reports it unavailable — no silent `balanced` — its admitted Attempt keeps
/// the frozen `fast` invocation through a Tool continuation, new work is
/// refused explicitly, and selecting `deep` through `session/setModel`
/// recovers without preparation, a resource rebuild, a new Session or a
/// reconnect.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue459_removed_selected_profile_is_reported_until_explicit_recovery() {
    use crate::local_runtime::configuration::application::UnitApplication;
    use crate::model::catalog::{ModelProfileId, ModelRef};
    use crate::model::session::SessionModelConfig;
    use serde_json::json;
    use std::sync::atomic::Ordering;
    bounded(Box::pin(async {
        let f = Fixture::with_session_count(Some("read"), 1).await;
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        let fast = ("fast", json!({"temperature": 1.0, "top_p": 0.5}));
        let balanced = ("balanced", json!({"temperature": 0.5}));
        let deep = ("deep", json!({"temperature": 0.1, "seed": 42}));
        write_profiles(
            &connection,
            100,
            "balanced",
            &[fast.clone(), balanced.clone(), deep.clone()],
        )
        .await;
        let target = attach(&connection, &f, 0).await;
        let pinned = |profile: &str| SessionModelConfig {
            profile: Some(ModelProfileId::new(profile)),
            ..SessionModelConfig::of(ModelRef::parse("local/a").unwrap())
        };
        let set = |profile: &str| Method::ModelSet {
            target: target.clone(),
            config: Box::new(pinned(profile)),
        };
        call(&connection, 102, set("fast")).await;
        f.manager
            .reconcile_configuration(
                &crate::local_runtime::configuration::settings::SourceTarget::Workspace {
                    directory: f.workspaces[0].clone(),
                },
            )
            .await
            .unwrap();
        super::super::configuration::settled(&f, 0).await;
        let managed = f.manager.load(&target.session_id, None).await.unwrap();
        let runtime = f.manager.configuration_runtime(&target.session_id).unwrap();
        let probe = f.manager.probe(&f.sessions[0].active_conversation_id);
        let resources = runtime.runtime_resources();
        let preparations = probe.configuration_preparations.load(Ordering::SeqCst);
        let selected = runtime.model_view();
        assert!(selected.unavailable.is_none());

        let settlement = runtime.settlement_signal();
        call(
            &connection,
            103,
            Method::TurnStart {
                target: target.clone(),
                content: vec![UserInputBlock::Text(crate::message::content::TextBlock {
                    text: "request-A".into(),
                })],
            },
        )
        .await;
        f.gates[0].wait_entered().await;
        let admitted = managed.client().checkpoint().unwrap().snapshot.attempt.unwrap().model;

        // `fast`, the pinned Profile, is removed from a valid catalog.
        write_profiles(&connection, 104, "balanced", &[balanced.clone(), deep.clone()]).await;
        let application = super::super::configuration::settled(&f, 0).await;
        assert!(application.candidate.is_none(), "{application:?}");
        assert!(
            application
                .units
                .values()
                .all(|unit| *unit == UnitApplication::Applied),
            "{application:?}"
        );
        assert_eq!(
            probe.configuration_preparations.load(Ordering::SeqCst),
            preparations
        );
        let published = runtime.runtime_resources();
        assert!(std::sync::Arc::ptr_eq(
            resources.capability(),
            published.capability()
        ));
        assert_eq!(published.revision(), resources.revision().next());

        // Discovery: both native surfaces advertise the published catalog.
        let catalog = read_catalog_result(call(
            &connection,
            106,
            Method::ModelCatalog {
                target: target.clone(),
            },
        )
        .await);
        assert_eq!(profile_ids(&catalog), ["balanced", "deep"]);
        let client = read_native_catalog_result(f
            .manager
            .configuration_host(&target.session_id)
            .unwrap()
            .model_catalog()
            .unwrap());
        assert_eq!(client, catalog);

        // The configured identity is still `fast`, reported unavailable; the
        // invocation it last resolved is retained, never replaced by a default.
        let model = read_model_result(
            &connection, &target, call(
            &connection,
            107,
            Method::ModelGet {
                target: target.clone(),
            },
        )
        .await).await;
        assert_eq!(model.configured, pinned("fast"));
        let diagnostic = model.unavailable.clone().expect("reported unavailable");
        assert!(diagnostic.contains("fast"), "{diagnostic}");
        assert_eq!(model.effective, selected.effective);
        assert_eq!(model.summary, selected.summary);
        assert_eq!(
            managed.client().checkpoint().unwrap().snapshot.attempt.unwrap().model,
            admitted
        );

        // The admitted Attempt finishes on its frozen `fast` invocation.
        f.gates[0].release();
        settlement.notified().await;
        let bodies = f.provider.request_bodies();
        assert_eq!(bodies.len(), 2, "Tool proposal and its continuation");
        for body in &bodies {
            let body: serde_json::Value = serde_json::from_str(body).unwrap();
            assert_eq!((&body["temperature"], &body["top_p"]), (&json!(1.0), &json!(0.5)));
        }

        // New work is refused explicitly and nothing reaches the provider.
        let refused = refusal(
            &connection,
            108,
            Method::TurnStart {
                target: target.clone(),
                content: vec![UserInputBlock::Text(crate::message::content::TextBlock {
                    text: "request-B".into(),
                })],
            },
        )
        .await;
        assert!(
            matches!(&refused, ErrorData::ModelUnavailable { diagnostic: refused } if *refused == format!("the configured session model is unavailable: {diagnostic}")),
            "{refused:?}"
        );
        assert!(matches!(
            refusal(
                &connection,
                109,
                Method::CompactContext {
                    target: target.clone(),
                    request_id: "issue459".to_owned().try_into().unwrap(),
                },
            )
            .await,
            ErrorData::ModelUnavailable { .. }
        ));
        assert!(managed.client().checkpoint().unwrap().snapshot.inbound.pending.is_empty());
        assert_eq!(f.provider.request_bodies().len(), 2);
        // Re-selecting the removed Profile is refused by the published catalog.
        assert!(matches!(
            refusal(&connection, 110, set("fast")).await,
            ErrorData::ModelUnavailable { .. }
        ));
        assert_eq!(runtime.model_view(), *model);

        // Explicit recovery uses the published catalog without an idle gate.
        let MethodResult::ModelSet { .. } = call(&connection, 111, set("deep")).await else {
            panic!("bounded selection receipt")
        };
        let model = read_model_result(
            &connection, &target, call(&connection, 10_108, Method::ModelGet {target:target.clone()}).await).await;
        assert!(model.unavailable.is_none());
        assert_eq!(
            model.effective.as_ref().unwrap().profile,
            Some(ModelProfileId::new("deep"))
        );
        let settlement = runtime.settlement_signal();
        call(
            &connection,
            112,
            Method::TurnStart {
                target: target.clone(),
                content: vec![UserInputBlock::Text(crate::message::content::TextBlock {
                    text: "request-C".into(),
                })],
            },
        )
        .await;
        settlement.notified().await;
        let bodies = f.provider.request_bodies();
        assert_eq!(bodies.len(), 3);
        let body: serde_json::Value = serde_json::from_str(&bodies[2]).unwrap();
        assert_eq!(
            (&body["temperature"], &body["seed"]),
            (&json!(0.1), &json!(42))
        );
        assert!(body.get("top_p").is_none());
        // Recovery rebuilt nothing: the same executable tool registry.
        assert!(std::sync::Arc::ptr_eq(
            resources.capability().tool_registry(),
            runtime.runtime_resources().capability().tool_registry()
        ));
        assert_eq!(
            probe.configuration_preparations.load(Ordering::SeqCst),
            preparations
        );
        f.close().await;
    }))
    .await;
}

/// Issue #459: the acceptance sequence. An existing Session uses `local/a`; a
/// valid published catalog removes it, introduces `local/c` and makes that
/// the new-Session default. The Session keeps `local/a` configured and
/// unavailable — nothing switches it — discovers `local/c`, refuses `local/a`
/// as a new explicit selection, and after `session/setModel` the next Attempt
/// sends `local/c`'s wire identity and exact Profile parameters, all on the
/// same runtime incarnation and attachment.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue459_removed_selected_model_is_replaced_explicitly_in_place() {
    use crate::local_runtime::configuration::application::UnitApplication;
    use crate::local_runtime::configuration::settings::SourceTarget;
    use crate::model::catalog::ModelRef;
    use crate::model::session::SessionModelConfig;
    use serde_json::json;
    use std::sync::atomic::Ordering;
    bounded(Box::pin(async {
        let f = Fixture::with_session_count(None, 1).await;
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        let target = attach(&connection, &f, 0).await;
        let model = |id: &str| SessionModelConfig::of(ModelRef::parse(id).unwrap());
        let set = |id: &str| Method::ModelSet {
            target: target.clone(),
            config: Box::new(model(id)),
        };
        // Persist the Session's currently resolved `local/a` as explicit intent.
        call(&connection, 100, set("local/a")).await;
        let managed = f.manager.load(&target.session_id, None).await.unwrap();
        let incarnation = managed.incarnation_id();
        let runtime = f.manager.configuration_runtime(&target.session_id).unwrap();
        let probe = f.manager.probe(&f.sessions[0].active_conversation_id);
        let compositions = probe.compositions.load(Ordering::SeqCst);
        let preparations = probe.configuration_preparations.load(Ordering::SeqCst);
        let resources = runtime.runtime_resources();
        let selected = runtime.model_view();

        // One valid source generation removes `local/a`, adds `local/c` with
        // its own wire identity and Profile, and moves the default to it.
        let source = f
            .manager
            .source_settings(&SourceTarget::User)
            .await
            .unwrap();
        let mut document: toml::Value =
            toml::from_str(&std::fs::read_to_string(&source.user.path).unwrap()).unwrap();
        let models = document["models"].as_table_mut().unwrap();
        let mut replacement = models.remove("local/a").unwrap();
        let table = replacement.as_table_mut().unwrap();
        table.remove("request_params");
        table.insert("id".into(), "c".into());
        table.insert("default_profile".into(), "steady".into());
        table.insert(
            "profiles".into(),
            toml::toml! { steady = { request_params = "{\"temperature\":0.25,\"seed\":7}" } }
                .into(),
        );
        models.insert("local/c".into(), replacement);
        document["agent"]["model"]["model"] = "local/c".into();
        std::fs::write(&source.user.path, toml::to_string(&document).unwrap()).unwrap();
        call(
            &connection,
            101,
            Method::ConfigurationReconcile {
                target: SourceTarget::User,
            },
        )
        .await;
        let application = super::super::configuration::settled(&f, 0).await;
        assert!(application.candidate.is_none(), "{application:?}");
        assert!(
            application
                .units
                .values()
                .all(|unit| *unit == UnitApplication::Applied),
            "{application:?}"
        );
        assert_eq!(
            probe.configuration_preparations.load(Ordering::SeqCst),
            preparations
        );
        let published = runtime.runtime_resources();
        assert!(std::sync::Arc::ptr_eq(
            resources.capability(),
            published.capability()
        ));
        assert_eq!(published.revision(), resources.revision().next());
        // The new-Session default travels with the catalog it is valid in.
        assert_eq!(
            runtime
                .configuration_view()
                .unwrap()
                .root_agent
                .model
                .unwrap()
                .model
                .to_string(),
            "local/c"
        );

        // The Session still asks for `local/a`, reported unavailable.
        let view = read_model_result(
            &connection,
            &target,
            call(
                &connection,
                102,
                Method::ModelGet {
                    target: target.clone(),
                },
            )
            .await,
        )
        .await;
        assert_eq!(view.configured, model("local/a"));
        assert!(view.unavailable.as_deref().unwrap().contains("local/a"));
        assert_eq!(view.effective, selected.effective);
        // Discovery advertises the replacement, and only the published catalog.
        let catalog = read_catalog_result(
            call(
                &connection,
                103,
                Method::ModelCatalog {
                    target: target.clone(),
                },
            )
            .await,
        );
        let references: Vec<_> = catalog
            .models
            .iter()
            .map(|entry| entry.model.to_string())
            .collect();
        assert_eq!(references, ["local/b", "local/c"]);
        // `local/a` is no longer admitted as an explicit selection, and no
        // turn runs on it.
        assert!(matches!(
            refusal(&connection, 104, set("local/a")).await,
            ErrorData::ModelUnavailable { .. }
        ));
        assert!(matches!(
            refusal(
                &connection,
                105,
                Method::TurnStart {
                    target: target.clone(),
                    content: vec![UserInputBlock::Text(crate::message::content::TextBlock {
                        text: "on a".into(),
                    })],
                },
            )
            .await,
            ErrorData::ModelUnavailable { .. }
        ));
        assert!(f.provider.request_bodies().is_empty());

        // Explicit selection of the replacement, then the next Attempt.
        assert!(matches!(
            call(&connection, 106, set("local/c")).await,
            MethodResult::ModelSet { .. }
        ));
        let view = read_model_result(
            &connection,
            &target,
            call(
                &connection,
                10_106,
                Method::ModelGet {
                    target: target.clone(),
                },
            )
            .await,
        )
        .await;
        assert!(view.unavailable.is_none());
        let effective = view.effective.as_ref().unwrap();
        assert_eq!(effective.model.to_string(), "local/c");
        assert_eq!(effective.profile.as_ref().unwrap().as_str(), "steady");
        f.gates[0].release();
        let settlement = runtime.settlement_signal();
        call(
            &connection,
            107,
            Method::TurnStart {
                target: target.clone(),
                content: vec![UserInputBlock::Text(crate::message::content::TextBlock {
                    text: "on c".into(),
                })],
            },
        )
        .await;
        settlement.notified().await;
        let bodies = f.provider.request_bodies();
        assert_eq!(bodies.len(), 1);
        let body: serde_json::Value = serde_json::from_str(&bodies[0]).unwrap();
        assert_eq!(
            (&body["model"], &body["temperature"], &body["seed"]),
            (&json!("c"), &json!(0.25), &json!(7))
        );
        // No reload, recomposition or reattachment was involved.
        let managed = f.manager.load(&target.session_id, None).await.unwrap();
        assert_eq!(managed.incarnation_id(), incarnation);
        assert_eq!(probe.compositions.load(Ordering::SeqCst), compositions);
        call(
            &connection,
            108,
            Method::ModelGet {
                target: target.clone(),
            },
        )
        .await;
        f.close().await;
    }))
    .await;
}
