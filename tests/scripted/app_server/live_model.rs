//! Issue #468: exercise durable native selection through the public protocol.
//! Loopback response-header gates establish admission before selection; the
//! native observation stream establishes settlement before subsequent work.
use super::*;
use crate::model::catalog::{ModelProfileId, ModelRef};
use crate::model::session::{SessionModelConfig, SummaryModelPolicy};
use serde_json::json;

fn selection(model: &str) -> SessionModelConfig {
    SessionModelConfig::of(ModelRef::parse(model).unwrap())
}

async fn select(
    connection: &AppServerConnection,
    target: &AttachmentTarget,
    config: &SessionModelConfig,
) -> Box<crate::model::session::SessionModelView> {
    let MethodResult::ModelSet { .. } = call(
        connection,
        468,
        Method::ModelSet {
            target: target.clone(),
            config: Box::new(config.clone()),
        },
    )
    .await
    else {
        panic!("committed model selection")
    };
    let model = super::read_model_result(
        connection,
        target,
        call(
            connection,
            470,
            Method::ModelGet {
                target: target.clone(),
            },
        )
        .await,
    )
    .await;
    assert_eq!(&model.configured, config);
    model
}

async fn reattach(
    connection: &AppServerConnection,
    session_id: &crate::local_runtime::session::SessionId,
) -> (
    AttachmentTarget,
    Box<crate::runtime_client::RuntimeClientSnapshot>,
) {
    let MethodResult::Attached { target, .. } = call(
        connection,
        469,
        Method::SessionAttach {
            session_id: session_id.clone(),
            node_id: None,
        },
    )
    .await
    else {
        panic!("reattached")
    };
    let MethodResult::Checkpoint { checkpoint } = call(
        connection,
        470,
        Method::SessionCheckpoint {
            target: target.clone(),
            trace_records: Vec::new(),
        },
    )
    .await
    else {
        panic!("bounded reattachment checkpoint")
    };
    (target, Box::new(checkpoint.snapshot))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn selecting_the_implicit_current_model_persists_explicit_session_authority() {
    bounded(async {
        let f = Fixture::new().await;
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        let target = attach(&connection, &f, 0).await;
        assert!(
            f.manager
                .sessions
                .read_settings(&target.session_id)
                .await
                .unwrap()
                .1
                .model
                .is_none()
        );
        let current = f
            .manager
            .configuration_runtime(&target.session_id)
            .unwrap()
            .model_view()
            .configured;
        select(&connection, &target, &current).await;
        let committed = f
            .manager
            .sessions
            .read_settings(&target.session_id)
            .await
            .unwrap();
        assert_eq!(committed.1.model, Some(current.clone()));
        select(&connection, &target, &current).await;
        assert_eq!(
            f.manager
                .sessions
                .read_settings(&target.session_id)
                .await
                .unwrap(),
            committed,
            "repeated explicit selection is a durable no-op"
        );
        connection.close();
        f.manager.unload(&target.conversation_id).await.unwrap();
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        let (_, snapshot) = reattach(&connection, &target.session_id).await;
        assert_eq!(snapshot.model.unwrap().configured, current);
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn active_profile_selection_is_durable_and_reconnect_preserves_frozen_attempt() {
    bounded(Box::pin(async {
        let f = Fixture::with_tool(Some("read")).await;
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        write_profiles(
            &connection,
            100,
            "high",
            &[
                (
                    "high",
                    json!({"reasoning_effort": "high", "temperature": 0.1}),
                ),
                (
                    "low",
                    json!({"reasoning_effort": "low", "temperature": 0.8}),
                ),
            ],
        )
        .await;
        let target = attach(&connection, &f, 0).await;
        let high = SessionModelConfig {
            profile: Some(ModelProfileId::new("high")),
            request_params: crate::scripted_suites::common::request_params(json!({"seed": 7})),
            max_output_tokens: Some(777),
            summary_model: SummaryModelPolicy::Explicit {
                model: ModelRef::parse("local/b").unwrap(),
                profile: None,
                request_params: crate::scripted_suites::common::request_params(
                    json!({"temperature": 0.2}),
                ),
                max_output_tokens: Some(222),
            },
            ..selection("local/a")
        };
        select(&connection, &target, &high).await;
        let completion = AttemptCompletion::capture(&f, &target);
        call(
            &connection,
            102,
            Method::TurnStart {
                target: target.clone(),
                content: wire_text("request-A"),
            },
        )
        .await;
        f.gates[0].wait_entered().await;
        let managed = f.manager.load(&target.session_id, None).await.unwrap();
        let admitted = managed
            .client()
            .checkpoint()
            .unwrap()
            .snapshot
            .attempt
            .unwrap();
        let runtime = f.manager.configuration_runtime(&target.session_id).unwrap();
        let resources = runtime.runtime_resources();
        let low = SessionModelConfig {
            profile: Some(ModelProfileId::new("low")),
            ..high.clone()
        };
        let committed = select(&connection, &target, &low).await;
        assert!(
            std::sync::Arc::ptr_eq(&resources, &runtime.runtime_resources()),
            "selection retains physical resources and their existing leases"
        );
        assert_eq!(committed.effective.as_ref().unwrap().max_output_tokens, 777);
        assert_eq!(
            committed.effective.as_ref().unwrap().request_params["seed"],
            json!(7)
        );
        assert_eq!(
            f.manager
                .sessions
                .read_settings(&target.session_id)
                .await
                .unwrap()
                .1
                .model,
            Some(low.clone())
        );
        let during = managed.client().checkpoint().unwrap().snapshot;
        assert_eq!(during.model.as_ref().unwrap().configured, low);
        assert_eq!(during.attempt.as_ref().unwrap().model, admitted.model);

        // Invalid whole-state replacements never publish or persist a partial selection.
        let invalid = [
            SessionModelConfig {
                profile: Some(ModelProfileId::new("missing")),
                ..low.clone()
            },
            SessionModelConfig {
                max_output_tokens: Some(4097),
                ..low.clone()
            },
            SessionModelConfig {
                model: ModelRef::parse("local/missing").unwrap(),
                ..low.clone()
            },
        ];
        for config in invalid {
            assert!(matches!(
                refusal(
                    &connection,
                    103,
                    Method::ModelSet {
                        target: target.clone(),
                        config: Box::new(config),
                    }
                )
                .await,
                ErrorData::ModelUnavailable { .. }
            ));
            assert_eq!(
                managed
                    .client()
                    .checkpoint()
                    .unwrap()
                    .snapshot
                    .model
                    .as_ref()
                    .unwrap()
                    .configured,
                low
            );
            assert_eq!(
                f.manager
                    .sessions
                    .read_settings(&target.session_id)
                    .await
                    .unwrap()
                    .1
                    .model,
                Some(low.clone())
            );
        }

        // A fresh connection has no cached mutation acknowledgement to trust.
        connection.close();
        let recovered = AppServerConnection::new(f.host.clone());
        initialize(&recovered).await;
        let (target, snapshot) = reattach(&recovered, &target.session_id).await;
        assert_eq!(snapshot.model.as_ref().unwrap().configured, low);
        assert_eq!(snapshot.attempt.as_ref().unwrap().model, admitted.model);
        call(
            &recovered,
            104,
            Method::SessionDetach {
                target: target.clone(),
            },
        )
        .await;
        let (target, snapshot) = reattach(&recovered, &target.session_id).await;
        assert_eq!(snapshot.model.as_ref().unwrap().configured, low);
        assert_eq!(snapshot.attempt.as_ref().unwrap().model, admitted.model);

        f.gates[0].release();
        completion.observed(&recovered, &target).await;
        let bodies = f.provider.request_bodies();
        assert_eq!(bodies.len(), 2, "first turn and Tool continuation");
        for body in bodies {
            let body: serde_json::Value = serde_json::from_str(&body).unwrap();
            assert_eq!(body["model"], json!("a"));
            assert_eq!(body["reasoning_effort"], json!("high"));
            assert_eq!(body["seed"], json!(7));
            assert_eq!(body["max_completion_tokens"], json!(777));
        }
        recovered.close();
        f.manager.unload(&target.conversation_id).await.unwrap();
        let reloaded = AppServerConnection::new(f.host.clone());
        initialize(&reloaded).await;
        let (target, snapshot) = reattach(&reloaded, &target.session_id).await;
        assert_eq!(snapshot.model.as_ref().unwrap().configured, low);
        // A new runtime incarnation has no active Attempt; historical requests
        // retain model and Profile through the existing durable Trace authority.
        assert!(snapshot.attempt.is_none());
        let historical: Vec<_> = snapshot
            .trace
            .records
            .iter()
            .filter(|record| record.kind == crate::observation::trace::TraceKind::Request)
            .collect();
        assert_eq!(historical.len(), 2);
        for record in historical {
            let MethodResult::DomainPage { page } = call(
                &reloaded,
                106,
                Method::TraceDetail {
                    target: target.clone(),
                    record_id: record.id.clone(),
                },
            )
            .await
            else {
                panic!("historical request detail")
            };
            assert_eq!(page.items.len(), 1);
            let detail = page.items[0]
                .value
                .as_ref()
                .expect("finite native request header");
            let request = &detail["request"];
            assert_eq!(request["attempt_id"], json!(admitted.attempt_id));
            assert_eq!(request["model"], json!("a"));
            assert_eq!(request["profile"], json!("high"));
            assert_eq!(request["max_output_tokens"], json!(777));
        }
        let next_completion = AttemptCompletion::capture(&f, &target);
        call(
            &reloaded,
            105,
            Method::TurnStart {
                target: target.clone(),
                content: wire_text("next independent Attempt"),
            },
        )
        .await;
        next_completion.observed(&reloaded, &target).await;
        let bodies = f.provider.request_bodies();
        assert_eq!(bodies.len(), 3);
        let next: serde_json::Value = serde_json::from_str(&bodies[2]).unwrap();
        assert_eq!(next["reasoning_effort"], json!("low"));
        assert_eq!(next["temperature"], json!(0.8));
        assert_eq!(next["seed"], json!(7));
        assert_eq!(next["max_completion_tokens"], json!(777));
        f.close().await;
    }))
    .await;
}

pub(crate) async fn live_model_child_inheritance_boundary_scenario() {
    bounded(Box::pin(async {
        let f = Fixture::with_tool(Some("delivery-child")).await;
        let profile = f.workspaces[0].join(".agents/agents/reviewer.toml");
        std::fs::create_dir_all(profile.parent().unwrap()).unwrap();
        std::fs::write(
            &profile,
            "description='Model inheritance child'\ninstructions='Report success'\n",
        )
        .unwrap();
        std::fs::write(
            f.workspaces[0].join("rustx.toml"),
            "[agent]\nagents=['reviewer']\n",
        )
        .unwrap();
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        let target = attach(&connection, &f, 0).await;
        let original = SessionModelConfig {
            request_params: crate::scripted_suites::common::request_params(
                json!({"temperature": 0.1}),
            ),
            max_output_tokens: Some(333),
            ..selection("local/a")
        };
        select(&connection, &target, &original).await;
        let runtime = f.manager.configuration_runtime(&target.session_id).unwrap();
        let parent_resources = runtime.runtime_resources();
        let completion = AttemptCompletion::capture(&f, &target);
        call(
            &connection,
            110,
            Method::TurnStart {
                target: target.clone(),
                content: wire_text("request-A"),
            },
        )
        .await;
        f.gates[0].wait_entered().await;
        let replacement = SessionModelConfig {
            request_params: crate::scripted_suites::common::request_params(
                json!({"temperature": 0.9}),
            ),
            max_output_tokens: Some(999),
            ..selection("local/b")
        };
        select(&connection, &target, &replacement).await;
        assert!(std::sync::Arc::ptr_eq(
            &parent_resources,
            &runtime.runtime_resources()
        ));
        f.gates[0].release();
        f.gates[1].wait_entered().await;
        completion.observed(&connection, &target).await;
        let registry = runtime.subagent_registry().unwrap();
        assert!(
            registry.configuration_busy(),
            "the child still owns physical execution"
        );
        let children = registry.all_snapshots();
        assert_eq!(children.len(), 1);
        let bodies = f.provider.request_bodies();
        assert_eq!(
            bodies.len(),
            3,
            "parent Tool turn, parent continuation and child turn"
        );
        for body in &bodies {
            let body: serde_json::Value = serde_json::from_str(body).unwrap();
            assert_eq!(body["model"], json!("a"));
            assert_eq!(body["temperature"], json!(0.1));
            assert_eq!(body["max_completion_tokens"], json!(333));
        }
        // The same native mutation remains available with only a child running.
        let changed = SessionModelConfig {
            max_output_tokens: Some(888),
            ..replacement
        };
        select(&connection, &target, &changed).await;
        assert!(registry.configuration_busy());
        assert_eq!(
            registry.all_snapshots()[0].subagent_id,
            children[0].subagent_id
        );
        let next_completion = AttemptCompletion::capture(&f, &target);
        call(
            &connection,
            111,
            Method::TurnStart {
                target: target.clone(),
                content: wire_text("independent request"),
            },
        )
        .await;
        next_completion.observed(&connection, &target).await;
        let bodies = f.provider.request_bodies();
        assert_eq!(bodies.len(), 4);
        let next: serde_json::Value = serde_json::from_str(&bodies[3]).unwrap();
        assert_eq!(next["model"], json!("b"));
        assert_eq!(next["temperature"], json!(0.9));
        assert_eq!(next["max_completion_tokens"], json!(888));
        f.gates[1].release();
        f.close().await;
    }))
    .await;
}
