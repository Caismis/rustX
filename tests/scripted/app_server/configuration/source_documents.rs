//! Source document semantics on the wire: JSON-string round trips, effective
//! invocation, exact numeric literals judged before rounding, profile key
//! collisions and following versus pinned default profiles.
#![allow(clippy::large_futures)] // bounded fixture futures; no recursive or unbounded stack growth
use super::*;

/// Issue #456: App Server JSON mutations carry structured `request_params`;
/// the native CAS writer persists every one of them — Model, Profiles, Root
/// selection overrides, explicit Summary overrides and a named Agent's
/// selection — as JSON-encoded strings, and the reread is the same document.
/// Reformatting a JSON string (whitespace and key order) is semantically
/// inert: it publishes no adoption candidate and changes no runtime state.
#[allow(clippy::too_many_lines)] // one complete authoring round trip
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue456_json_string_sources_round_trip_and_formatting_is_semantically_inert() {
    use crate::local_runtime::authoring::{
        ModelLayer, ModelOutput, RuntimeLayer, SummaryAuthoring,
    };
    use crate::local_runtime::configuration::settings::SourceTarget;
    use crate::model::catalog::{ModelProfileId, ModelRef};
    use crate::toml_authoring::AuthoredRequestParams;
    let params =
        |value: serde_json::Value| AuthoredRequestParams(serde_json::from_value(value).unwrap());
    Box::pin(bounded(async {
        let fixture = Fixture::with_session_count(None, 1).await;
        let id = &fixture.sessions[0].id;
        fixture.manager.load(id, None).await.unwrap();
        let models = fixture
            .manager
            .source_settings(&SourceTarget::User)
            .await
            .unwrap()
            .user
            .authored
            .unwrap()
            .models
            .unwrap();
        // A Model without profiles keeps its own native object, null included.
        let mut a = models["local/a"].clone();
        a.request_params = Some(params(serde_json::json!({"top_p": 0.5, "unset": null})));
        write(&fixture, 0, ConfigMutation::Model { id: "local/a".into(), authored: Some(a) }).await;
        // A Model with independent, complete profiles.
        let mut b = models["local/b"].clone();
        b.request_params = None;
        b.default_profile = Some(ModelProfileId::new("precise"));
        b.profiles = Some(
            [
                ("precise", Some(256), serde_json::json!({"temperature": 0.1, "stop": [null], "nested": {"a": [1, {"b": null}]}})),
                ("creative", None, serde_json::json!({"temperature": 1.3})),
            ]
            .into_iter()
            .map(|(name, budget, value)| {
                (
                    ModelProfileId::new(name),
                    crate::model::authoring::Profile {
                        reasoning_enabled: None,
                        max_output_tokens: budget,
                        request_params: params(value),
                    },
                )
            })
            .collect(),
        );
        write(&fixture, 0, ConfigMutation::Model { id: "local/b".into(), authored: Some(b) }).await;
        // Root selection overrides and explicit Summary overrides.
        let root = ModelLayer {
            model: Some(ModelRef::parse("local/b").unwrap()),
            profile: Some(ModelProfileId::new("creative")),
            request_params: Some(params(serde_json::json!({"seed": null}))),
            max_output_tokens: None,
            summary_model: Some(SummaryAuthoring::Explicit {
                model: ModelRef::parse("local/a").unwrap(),
                profile: None,
                request_params: params(serde_json::json!({"summary_tag": [null]})),
                max_output_tokens: Some(ModelOutput::Limit { tokens: 128 }),
            }),
        };
        write(&fixture, 0, ConfigMutation::RootModel { authored: Some(root) }).await;
        // A named Agent's own selection.
        let source = fixture.manager.source_settings(&SourceTarget::User).await.unwrap();
        let mut agent_model = crate::model::session::SessionModelConfig::of(ModelRef::parse("local/b").unwrap());
        agent_model.profile = Some(ModelProfileId::new("precise"));
        agent_model.request_params = params(serde_json::json!({"metadata": {"k": null}})).0;
        let agent = crate::local_runtime::config::AgentProfileDocument {
            description: "Profiles".into(),
            model: Some(agent_model.clone()),
            ..Default::default()
        };
        fixture
            .manager
            .write_source_settings(&SourceTarget::User, source.absent_resource_revision.clone(), SourceMutation::Agent {
                        name: crate::runtime::subagent::SubagentName::parse("profiled").unwrap(),
                        authored: Some(agent.clone()),
                    })
            .await
            .unwrap();

        // Native source TOML: every request_params is a JSON-encoded string.
        let source = fixture.manager.source_settings(&SourceTarget::User).await.unwrap();
        let text = std::fs::read_to_string(&source.user.path).unwrap();
        let table = text.parse::<toml::Table>().unwrap();
        let json = |value: &toml::Value| -> serde_json::Value {
            serde_json::from_str(value.as_str().expect("a JSON-encoded string")).unwrap()
        };
        assert_eq!(json(&table["models"]["local/a"]["request_params"]), serde_json::json!({"top_p": 0.5, "unset": null}));
        assert!(table["models"]["local/b"].get("request_params").is_none());
        assert_eq!(
            json(&table["models"]["local/b"]["profiles"]["precise"]["request_params"]),
            serde_json::json!({"temperature": 0.1, "stop": [null], "nested": {"a": [1, {"b": null}]}})
        );
        assert_eq!(json(&table["agent"]["model"]["request_params"]), serde_json::json!({"seed": null}));
        assert_eq!(
            json(&table["agent"]["model"]["summary_model"]["request_params"]),
            serde_json::json!({"summary_tag": [null]})
        );
        let agent_view = source.agents.iter().find(|view| view.name.as_str() == "profiled").unwrap();
        let agent_text = std::fs::read_to_string(&agent_view.source.path).unwrap();
        let agent_table = agent_text.parse::<toml::Table>().unwrap();
        assert_eq!(json(&agent_table["model"]["request_params"]), serde_json::json!({"metadata": {"k": null}}));
        assert_eq!(agent_view.source.authored.as_ref().unwrap(), &agent);
        // The reread source is exactly the authored document; the App Server
        // projection of it is structured JSON.
        let reread: RuntimeLayer = crate::toml_authoring::parse(text.as_bytes()).unwrap();
        let projected = source.user.authored.as_ref().unwrap();
        assert_eq!(reread.models, projected.models);
        assert_eq!(reread.agent, projected.agent);
        let wire = serde_json::to_value(&source).unwrap();
        assert_eq!(
            wire["user"]["authored"]["models"]["local/b"]["profiles"]["precise"]["request_params"],
            serde_json::json!({"temperature": 0.1, "stop": [null], "nested": {"a": [1, {"b": null}]}})
        );
        assert_eq!(wire["user"]["authored"]["agent"]["model"]["profile"], "creative");

        // Reach a settled baseline for the existing Session.
        let application = settled(&fixture, 0).await;
        if let Some(candidate) = application.candidate {
            fixture.manager.adopt_configuration(id, &candidate.identity, candidate.expected_binding).unwrap();
        }
        let application = settled(&fixture, 0).await;
        assert!(application.candidate.is_none(), "{application:?}");
        let runtime = fixture.manager.configuration_runtime(id).unwrap();
        let resources = runtime.runtime_resources();
        let model = runtime.model_view();
        let resolved = source.resolved.clone();

        // Reformat every JSON string: whitespace and key order only.
        let mut reformatted = text.clone();
        for (compact, pretty) in [
            (r#"{"top_p":0.5,"unset":null}"#, "{ \"unset\" : null,\n  \"top_p\": 0.5 }"),
            (
                r#"{"temperature":0.1,"stop":[null],"nested":{"a":[1,{"b":null}]}}"#,
                "{\n  \"temperature\": 0.1,\n  \"stop\": [ null ],\n  \"nested\": { \"a\": [ 1, { \"b\": null } ] }\n}",
            ),
            (r#"{"seed":null}"#, "{ \"seed\": null }"),
            (r#"{"summary_tag":[null]}"#, "{\n\"summary_tag\" : [null]\n}"),
        ] {
            let quoted = format!("'{compact}'");
            assert!(reformatted.contains(&quoted), "{compact}: {reformatted}");
            reformatted = reformatted.replace(&quoted, &format!("'''{pretty}'''"));
        }
        assert_ne!(reformatted, text);
        std::fs::write(&source.user.path, &reformatted).unwrap();
        fixture
            .manager
            .reconcile_configuration(&SourceTarget::Workspace { directory: fixture.workspaces[0].clone() })
            .await
            .unwrap();
        let application = settled(&fixture, 0).await;
        assert!(application.candidate.is_none(), "{application:?}");
        assert!(Arc::ptr_eq(&resources, &runtime.runtime_resources()));
        assert_eq!(runtime.model_view(), model);
        let after = fixture.manager.source_settings(&SourceTarget::User).await.unwrap();
        assert_ne!(after.user.revision, source.user.revision);
        assert_eq!(after.resolved, resolved);
        assert_eq!(after.user.authored.unwrap().models, projected.models);

        // A new Session resolves the authored selection end to end.
        let created = fixture
            .manager
            .create_session(SessionPersistentState { cwd: fixture.workspaces[0].clone(), model: None })
            .await
            .unwrap();
        fixture.manager.load(&created.session.id, None).await.unwrap();
        let view = fixture.manager.configuration_runtime(&created.session.id).unwrap().model_view();
        assert_eq!(view.effective.as_ref().unwrap().profile, Some(ModelProfileId::new("creative")));
        assert_eq!(
            serde_json::Value::Object(view.effective.as_ref().unwrap().request_params.clone()),
            serde_json::json!({"temperature": 1.3, "seed": null})
        );
        let Some(crate::model::session::SummaryModelView::Explicit(summary)) = view.summary else {
            panic!("explicit summary");
        };
        assert_eq!(summary.max_output_tokens, 128);
        assert_eq!(
            serde_json::Value::Object(summary.request_params.clone()),
            serde_json::json!({"top_p": 0.5, "unset": null, "summary_tag": [null]})
        );
        fixture.close().await;
    }))
    .await;
}

/// Issue #456: whether an existing Session prepares and adopts a configuration
/// is decided by the effective invocation an Attempt would freeze, not by how
/// the selection is authored or by catalog entries it does not resolve
/// through. Catalog publication is separate: an edited unselected Profile is
/// what a later selection and a new Session resolve. Admitted Attempts keep
/// the invocation they froze, observed on the provider wire.
#[allow(clippy::too_many_lines)] // one ordered sequence of source edits
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue456_effective_invocation_decides_preparation_and_adoption() {
    use crate::local_runtime::configuration::settings::SourceTarget;
    use crate::model::authoring::Profile;
    use crate::model::catalog::{ModelProfileId, ModelRef};
    use crate::model::session::{SessionModelConfig, SummaryModelPolicy};
    use crate::toml_authoring::AuthoredRequestParams;
    use serde_json::json;
    let params =
        |value: serde_json::Value| AuthoredRequestParams(serde_json::from_value(value).unwrap());
    let object = |value: serde_json::Value| -> crate::model::invocation::RequestParams {
        serde_json::from_value(value).unwrap()
    };
    Box::pin(bounded(async {
        let fixture = Fixture::with_session_count(None, 1).await;
        let id = fixture.sessions[0].id.clone();
        fixture.manager.load(&id, None).await.unwrap();
        let runtime = fixture.manager.configuration_runtime(&id).unwrap();
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
        let model = |default: &str, profiles: [(&str, Option<u32>, serde_json::Value); 3]| {
            let mut model = authored.clone();
            model.request_params = None;
            model.default_profile = Some(ModelProfileId::new(default));
            model.profiles = Some(
                profiles
                    .into_iter()
                    .map(|(name, budget, value)| {
                        (
                            ModelProfileId::new(name),
                            Profile {
                                reasoning_enabled: None,
                                max_output_tokens: budget,
                                request_params: params(value),
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
        let balanced = ("balanced", None, json!({"temperature": 0.5}));
        let fast = ("fast", Some(64), json!({"temperature": 1.0}));
        let creative = ("creative", None, json!({"temperature": 1.2}));
        // The Session follows the Model's default Profile.
        write(
            &fixture,
            0,
            model(
                "balanced",
                [balanced.clone(), fast.clone(), creative.clone()],
            ),
        )
        .await;
        let candidate = settled(&fixture, 0)
            .await
            .candidate
            .expect("selected Model changed");
        fixture
            .manager
            .adopt_configuration(&id, &candidate.identity, candidate.expected_binding)
            .unwrap();
        assert!(settled(&fixture, 0).await.candidate.is_none());
        let view = runtime.model_view();
        assert_eq!(view.configured.profile, None);
        assert_eq!(
            view.effective.as_ref().unwrap().profile,
            Some(ModelProfileId::new("balanced"))
        );
        assert_eq!(
            view.effective.as_ref().unwrap().request_params,
            object(json!({"temperature": 0.5}))
        );
        // A catalog-only publication: nothing is prepared or adopted, the
        // resolved invocations and the capability snapshot are retained, and
        // one new generation carries the published catalog.
        let unchanged = |resources: &Arc<crate::runtime::resources::RuntimeResourceSnapshot>,
                         invocation: &crate::model::session::AttemptModelSnapshot,
                         previous: &ConfigurationApplication| {
            let application = fixture.manager.configuration_application(&id).unwrap();
            assert_ne!(
                application.desired, previous.desired,
                "a new source generation was applied"
            );
            assert!(application.candidate.is_none(), "{application:?}");
            assert!(
                application
                    .units
                    .values()
                    .all(|unit| *unit == UnitApplication::Applied),
                "{application:?}"
            );
            let published = runtime.runtime_resources();
            assert_eq!(published.revision(), resources.revision().next());
            assert!(Arc::ptr_eq(resources.capability(), published.capability()));
            assert_eq!(
                &runtime.model_snapshot(),
                invocation,
                "no invocation was re-resolved"
            );
        };

        // An unselected Profile edit publishes the catalog but neither
        // prepares nor adopts anything for the unchanged invocation.
        let resources = runtime.runtime_resources();
        let invocation = runtime.model_snapshot();
        let previous = fixture.manager.configuration_application(&id).unwrap();
        let fast_edited = ("fast", Some(32), json!({"temperature": 1.5, "top_p": 0.9}));
        write(
            &fixture,
            0,
            model(
                "balanced",
                [balanced.clone(), fast_edited.clone(), creative.clone()],
            ),
        )
        .await;
        settled(&fixture, 0).await;
        unchanged(&resources, &invocation, &previous);
        assert_eq!(runtime.model_view(), view);

        // An omitted and an explicit default Profile are one invocation: the
        // Session records the authored selection without rebuilding resources.
        let explicit = SessionModelConfig {
            profile: Some(ModelProfileId::new("balanced")),
            ..SessionModelConfig::of(ModelRef::parse("local/a").unwrap())
        };
        let revision = runtime.runtime_resources().revision();
        let adopted = fixture
            .manager
            .set_model(&id, explicit.clone())
            .await
            .unwrap()
            .0;
        assert_eq!(adopted.configured, explicit);
        assert_eq!(adopted.effective, view.effective);
        assert_eq!(runtime.runtime_resources().revision(), revision);
        // Moving the default no longer reaches this Session's explicit
        // selection; a new Session follows the moved default.
        let resources = runtime.runtime_resources();
        let invocation = runtime.model_snapshot();
        let previous = fixture.manager.configuration_application(&id).unwrap();
        write(
            &fixture,
            0,
            model(
                "fast",
                [balanced.clone(), fast_edited.clone(), creative.clone()],
            ),
        )
        .await;
        settled(&fixture, 0).await;
        unchanged(&resources, &invocation, &previous);
        assert_eq!(runtime.model_view().effective, view.effective);
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
            .unwrap()
            .model_view();
        assert_eq!(
            new.effective.as_ref().unwrap().profile,
            Some(ModelProfileId::new("fast"))
        );
        assert_eq!(
            new.effective.as_ref().unwrap().request_params,
            object(json!({"temperature": 1.5, "top_p": 0.9}))
        );
        assert_eq!(new.effective.as_ref().unwrap().max_output_tokens, 32);

        // An explicit Summary is compared on its own: editing the Profile only
        // the Summary resolves through needs preparation and adoption.
        let summarized = SessionModelConfig {
            summary_model: SummaryModelPolicy::Explicit {
                model: ModelRef::parse("local/a").unwrap(),
                profile: Some(ModelProfileId::new("creative")),
                request_params: serde_json::Map::new(),
                max_output_tokens: None,
            },
            ..explicit.clone()
        };
        fixture.manager.set_model(&id, summarized).await.unwrap();
        let resources = runtime.runtime_resources();
        let creative_edited = ("creative", None, json!({"temperature": 1.4}));
        write(
            &fixture,
            0,
            model(
                "fast",
                [
                    balanced.clone(),
                    fast_edited.clone(),
                    creative_edited.clone(),
                ],
            ),
        )
        .await;
        let candidate = settled(&fixture, 0)
            .await
            .candidate
            .expect("the Summary invocation changed");
        assert!(
            Arc::ptr_eq(&resources, &runtime.runtime_resources()),
            "adoption waits for its owner"
        );
        fixture
            .manager
            .adopt_configuration(&id, &candidate.identity, candidate.expected_binding)
            .unwrap();
        let Some(crate::model::session::SummaryModelView::Explicit(summary)) =
            runtime.model_view().summary
        else {
            panic!("explicit summary");
        };
        assert_eq!(summary.request_params, object(json!({"temperature": 1.4})));
        assert_eq!(runtime.model_view().effective, view.effective);

        // The selected Profile's parameters change while an Attempt is
        // admitted: the Attempt keeps its frozen invocation on the wire, and
        // the change is adopted for the next Attempt.
        let mut eligibility = EligibilityStream::open(&fixture, &id);
        let mut settlement = SettlementReceiptWait::capture(&runtime);
        runtime.submit_inbound(input("request-A frozen")).unwrap();
        fixture.gates[0].wait_entered().await;
        eligibility.until(AdoptionEligibility::Busy).await;
        let balanced_edited = ("balanced", None, json!({"temperature": 0.25, "seed": 7}));
        write(
            &fixture,
            0,
            model(
                "fast",
                [
                    balanced_edited.clone(),
                    fast_edited.clone(),
                    creative_edited.clone(),
                ],
            ),
        )
        .await;
        let candidate = settled(&fixture, 0)
            .await
            .candidate
            .expect("the selected Profile changed");
        assert_eq!(
            fixture.manager.adopt_configuration(
                &id,
                &candidate.identity,
                candidate.expected_binding
            ),
            Err(AdoptionError::Busy)
        );
        fixture.gates[0].release();
        settlement.wait().await;
        // Semantic settlement precedes release of the owning Attempt task.
        // Adoption needs the authoritative idle eligibility transition.
        eligibility.until(AdoptionEligibility::Eligible).await;
        let frozen: serde_json::Value =
            serde_json::from_str(&fixture.provider.request_body(0)).unwrap();
        assert_eq!(frozen["temperature"], json!(0.5));
        assert!(frozen.get("seed").is_none());
        fixture
            .manager
            .adopt_configuration(&id, &candidate.identity, candidate.expected_binding)
            .unwrap();
        let mut settlement = SettlementReceiptWait::capture(&runtime);
        runtime.submit_inbound(input("request-A adopted")).unwrap();
        fixture.gates[0].wait_entered().await;
        eligibility.until(AdoptionEligibility::Busy).await;
        fixture.gates[0].release();
        settlement.wait().await;
        eligibility.until(AdoptionEligibility::Eligible).await;
        let next: serde_json::Value =
            serde_json::from_str(&fixture.provider.request_body(1)).unwrap();
        assert_eq!(
            (&next["temperature"], &next["seed"]),
            (&json!(0.25), &json!(7))
        );

        // The selected Profile's output default alone is an effective change.
        write(
            &fixture,
            0,
            model(
                "fast",
                [
                    ("balanced", Some(128), balanced_edited.2.clone()),
                    fast_edited.clone(),
                    creative_edited.clone(),
                ],
            ),
        )
        .await;
        let candidate = settled(&fixture, 0)
            .await
            .candidate
            .expect("the output default changed");
        fixture
            .manager
            .adopt_configuration(&id, &candidate.identity, candidate.expected_binding)
            .unwrap();
        assert_eq!(
            runtime
                .model_view()
                .effective
                .as_ref()
                .unwrap()
                .max_output_tokens,
            128
        );

        // A provider binding change is never ignored, even with every
        // Profile untouched.
        let source = fixture
            .manager
            .source_settings(&SourceTarget::User)
            .await
            .unwrap();
        let provider = crate::local_runtime::configuration::settings::ProviderWrite {
            base_url: fixture.provider.url("/v2"),
            credential: crate::local_runtime::configuration::settings::CredentialEdit::Retain,
        };
        let resources = runtime.runtime_resources();
        fixture
            .manager
            .write_source_settings(
                &SourceTarget::User,
                source.user.revision,
                SourceMutation::Config {
                    mutation: ConfigMutation::Provider {
                        id: "local".into(),
                        authored: Some(provider),
                    },
                },
            )
            .await
            .unwrap();
        let application = settled(&fixture, 0).await;
        assert_eq!(
            application
                .candidate
                .expect("the provider endpoint changed")
                .impact,
            crate::model::request_shape::CacheImpact::CacheNamespaceChanged
        );
        assert!(Arc::ptr_eq(&resources, &runtime.runtime_resources()));
        fixture.close().await;
    }))
    .await;
}

/// Issue #456: request-parameter numbers are held to the binary64 domain every
/// hop shares. An App Server mutation carrying a number some hop would round
/// is refused before any native write; numbers inside the domain survive the
/// JSON mutation, the native JSON-string write, the reread and the client
/// projection exactly; and a hand-authored source outside the domain is
/// refused as a located diagnostic, so no editor can load and rewrite it.
#[allow(clippy::too_many_lines)] // one complete fidelity boundary
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue456_inexact_numbers_are_refused_before_any_write_and_exact_ones_round_trip() {
    use crate::local_runtime::authoring::ModelLayer;
    use crate::local_runtime::configuration::settings::SourceTarget;
    use crate::model::authoring::Profile;
    use crate::model::catalog::{ModelProfileId, ModelRef};
    use crate::toml_authoring::AuthoredRequestParams;
    use serde_json::json;
    let params =
        |value: serde_json::Value| AuthoredRequestParams(serde_json::from_value(value).unwrap());
    Box::pin(bounded(async {
        let f = Fixture::with_session_count(None, 0).await;
        let target = SourceTarget::User;
        let initial = f.manager.source_settings(&target).await.unwrap();
        let path = initial.user.path.clone();
        let original = std::fs::read(&path).unwrap();
        let authored = initial.user.authored.clone().unwrap().models.unwrap()["local/a"].clone();
        let profiled = |value: serde_json::Value| {
            let mut model = authored.clone();
            model.request_params = None;
            model.default_profile = Some(ModelProfileId::new("p"));
            model.profiles = Some(
                [(
                    ModelProfileId::new("p"),
                    Profile {
                        reasoning_enabled: None,
                        max_output_tokens: None,
                        request_params: params(value),
                    },
                )]
                .into(),
            );
            ConfigMutation::Model { id: "local/a".into(), authored: Some(model) }
        };
        let mutate = |mutation: ConfigMutation| {
            let revision = initial.user.revision.clone();
            let manager = f.manager.clone();
            async move {
                manager
                    .write_source_settings(&SourceTarget::User, revision, SourceMutation::Config { mutation })
                    .await
            }
        };
        // A structured mutation exactly as JSON client text carries it is
        // refused by the App Server wire decode, naming no value.
        let mut placeholder = authored.clone();
        placeholder.request_params = Some(params(json!({"seed": "PLACEHOLDER"})));
        let encoded = serde_json::to_string(&ConfigMutation::Model {
            id: "local/a".into(),
            authored: Some(placeholder),
        })
        .unwrap();
        for literal in ["9007199254740993", "-9007199254740993", "1152921504606846976"] {
            let error = serde_json::from_str::<ConfigMutation>(&encoded.replace("\"PLACEHOLDER\"", literal))
                .unwrap_err()
                .to_string();
            assert!(error.contains("request_params has a JSON number that is not exactly representable"), "{error}");
            assert!(!error.contains(literal.trim_start_matches('-')), "{error}");
        }
        // A structured value that bypassed the wire is refused by the native
        // writer, before any byte is written.
        for mutation in [
            ConfigMutation::Model {
                id: "local/a".into(),
                authored: Some(crate::model::authoring::Model {
                    request_params: Some(params(json!({"seed": 9_007_199_254_740_993_u64}))),
                    ..authored.clone()
                }),
            },
            profiled(json!({"nested": [{"seed": -9_007_199_254_740_993_i64}]})),
            profiled(json!({"seed": 1_152_921_504_606_846_976_u64})),
            ConfigMutation::RootModel {
                authored: Some(ModelLayer {
                    model: Some(ModelRef::parse("local/a").unwrap()),
                    profile: None,
                    request_params: Some(params(json!({"seed": 9_007_199_254_740_993_u64}))),
                    max_output_tokens: None,
                    summary_model: None,
                }),
            },
        ] {
            assert!(matches!(
                mutate(mutation).await,
                Err(crate::local_runtime::session_runtime_manager::SourceSettingsError::Source(
                    crate::local_runtime::configuration::settings::SettingsError::Invalid
                ))
            ));
            assert_eq!(std::fs::read(&path).unwrap(), original, "nothing was written");
        }

        // Exact numbers: the largest safe integers, ordinary and extreme
        // binary64 decimals, nested in objects and arrays.
        let exact = r#"{"seed":9007199254740992,"floor":-9007199254740991,"t":0.1,"huge":1e300,
            "tiny":5.357830195732913e-76,"nested":{"list":[1.5,{"n":-2.5e-7}]},"big":10000000000000000000}"#;
        let value: serde_json::Value = serde_json::from_str(exact).unwrap();
        let written = mutate(profiled(value.clone())).await.unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let table = text.parse::<toml::Table>().unwrap();
        let native = table["models"]["local/a"]["profiles"]["p"]["request_params"].as_str().unwrap();
        assert_eq!(crate::toml_authoring::parse_request_params_json(native).unwrap(), *value.as_object().unwrap());
        let reread = f.manager.source_settings(&target).await.unwrap();
        assert_eq!(reread.user.revision, written.revision);
        let projected = &reread.user.authored.as_ref().unwrap().models.as_ref().unwrap()["local/a"];
        assert_eq!(projected.profiles.as_ref().unwrap()[&ModelProfileId::new("p")].request_params.0, *value.as_object().unwrap());
        // The App Server response text, decoded the way a JSON client decodes it.
        let response = serde_json::to_string(&reread).unwrap();
        let client: serde_json::Value = serde_json::from_str(&response).unwrap();
        assert_eq!(client["user"]["authored"]["models"]["local/a"]["profiles"]["p"]["request_params"], value);
        assert!(response.contains("9007199254740992") && response.contains("5.357830195732913e-76"));

        // A hand-authored source outside the domain is a located diagnostic
        // that names the field, never the value or the neighboring credential;
        // an unrelated Model edit cannot load and rewrite it.
        let unsafe_text = text.replacen(
            native,
            "{\"seed\":9007199254740993}",
            1,
        ) + "\n[providers.extra]\nbase_url = \"https://example.invalid/v1\"\napi_key = \"sk-SECRET_NEIGHBOR\"\n";
        std::fs::write(&path, &unsafe_text).unwrap();
        let broken = f.manager.source_settings(&target).await.unwrap();
        assert!(broken.user.authored.is_none());
        let diagnostic = broken.user.diagnostic.clone().unwrap();
        assert!(diagnostic.contains("models.local/a.profiles.p.request_params has a JSON number that is not exactly representable"), "{diagnostic}");
        assert!(diagnostic.contains("(at line 1, column 9 of the JSON text)"), "{diagnostic}");
        assert!(!diagnostic.contains("9007199254740993") && !diagnostic.contains("SECRET"), "{diagnostic}");
        let mut unrelated = authored.clone();
        unrelated.context_window += 1;
        let refused = f
            .manager
            .write_source_settings(&target, broken.user.revision.clone(), SourceMutation::Config {
                        mutation: ConfigMutation::Model { id: "local/a".into(), authored: Some(unrelated) },
                    })
            .await;
        assert!(refused.is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), unsafe_text);
        assert!(f.provider.request_bodies().is_empty());
    }))
    .await;
}

/// The raw App Server request text of one typed call, with `"LITERAL"` spelled
/// as `literal`. The number exists only as text, as a client sends it: no
/// `Value` ever holds it rounded.
fn raw_request(call: crate::app_server::protocol::Method, literal: &str) -> String {
    use crate::app_server::protocol::{JsonRpcVersion, Request, RequestId};
    let text = serde_json::to_string(&Request {
        jsonrpc: JsonRpcVersion::V2,
        id: RequestId::Integer(456),
        call,
    })
    .unwrap();
    assert!(text.contains("\"LITERAL\""));
    text.replace("\"LITERAL\"", literal)
}

/// Issue #456: a provider-native parameter number is judged on the raw App
/// Server request text, before `serde_json` rounds it to binary64. Every
/// parameter entry point — Model and Profile authoring, the root and an
/// explicit Summary `ModelLayer`, a named Agent, and a Session selection and
/// its explicit Summary — refuses a literal some hop would round with a
/// key-free Invalid params, before any write, publication, Session change or
/// provider request; exact literals mean exactly what they spell from the raw
/// request through the native source, the reread projection and the provider
/// wire.
#[allow(clippy::too_many_lines)] // one complete raw-literal boundary
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue456_raw_request_param_literals_are_judged_before_any_rounding() {
    use crate::app_server::{connection::AppServerConnection, protocol::*};
    use crate::local_runtime::authoring::{ModelLayer, SummaryAuthoring};
    use crate::local_runtime::config::AgentProfileDocument;
    use crate::local_runtime::configuration::settings::SourceTarget;
    use crate::model::authoring::Profile;
    use crate::model::catalog::{ModelProfileId, ModelRef};
    use crate::model::session::{SessionModelConfig, SummaryModelPolicy};
    use crate::toml_authoring::AuthoredRequestParams;
    use serde_json::json;
    // Every entry point carries the literal at top level and nested in an
    // array of objects, beside a key that must never be echoed.
    let holes = || -> crate::model::invocation::RequestParams {
        serde_json::from_value(json!({
            "sk-SECRET_KEY": "LITERAL",
            "nested": [1, {"deep": ["LITERAL"]}]
        }))
        .unwrap()
    };
    Box::pin(bounded(async {
        let f = Fixture::with_session_count(None, 1).await;
        let connection = AppServerConnection::new(f.host.clone());
        super::super::protocol::initialize(&connection).await;
        let target = super::super::protocol::attach(&connection, &f, 0).await;
        let id = f.sessions[0].id.clone();
        let runtime = f.manager.configuration_runtime(&id).unwrap();
        let user = SourceTarget::User;
        let source = f.manager.source_settings(&user).await.unwrap();
        let path = source.user.path.clone();
        let original = std::fs::read(&path).unwrap();
        let authored = source.user.authored.clone().unwrap().models.unwrap()["local/a"].clone();
        let local = ModelRef::parse("local/a").unwrap();
        let profiled = |params: AuthoredRequestParams| {
            let mut model = authored.clone();
            model.request_params = None;
            model.default_profile = Some(ModelProfileId::new("p"));
            model.profiles = Some(
                [(
                    ModelProfileId::new("p"),
                    Profile { reasoning_enabled: None, max_output_tokens: None, request_params: params },
                )]
                .into(),
            );
            model
        };
        let write = |mutation: ConfigMutation| Method::SourcesWrite {
            target: user.clone(),
            expected_revision: source.user.revision.clone(),
            mutation: SourceMutation::Config { mutation },
        };
        let layer = |request_params, summary_model| ModelLayer {
            model: Some(local.clone()),
            profile: None,
            request_params,
            max_output_tokens: None,
            summary_model,
        };
        let selection = SessionModelConfig { request_params: holes(), ..SessionModelConfig::of(local.clone()) };
        let entry_points = [
            write(ConfigMutation::Model {
                id: "local/a".into(),
                authored: Some(crate::model::authoring::Model {
                    request_params: Some(AuthoredRequestParams(holes())),
                    ..authored.clone()
                }),
            }),
            write(ConfigMutation::Model { id: "local/a".into(), authored: Some(profiled(AuthoredRequestParams(holes()))) }),
            write(ConfigMutation::RootModel { authored: Some(layer(Some(AuthoredRequestParams(holes())), None)) }),
            write(ConfigMutation::RootModel {
                authored: Some(layer(
                    None,
                    Some(SummaryAuthoring::Explicit {
                        model: local.clone(),
                        profile: None,
                        request_params: AuthoredRequestParams(holes()),
                        max_output_tokens: None,
                    }),
                )),
            }),
            Method::SourcesWrite {
                target: user.clone(),
                expected_revision: source.user.revision.clone(),
                mutation: SourceMutation::Agent {
                    name: crate::runtime::subagent::SubagentName::parse("reviewer").unwrap(),
                    authored: Some(AgentProfileDocument {
                        description: "Reviews".into(),
                        instructions: "Review.".into(),
                        model: Some(selection.clone()),
                        ..AgentProfileDocument::default()
                    }),
                },
            },
            Method::ModelSet { target: target.clone(), config: Box::new(selection.clone()) },
            Method::ModelSet {
                target: target.clone(),
                config: Box::new(SessionModelConfig {
                    summary_model: SummaryModelPolicy::Explicit {
                        model: local.clone(),
                        profile: None,
                        request_params: holes(),
                        max_output_tokens: None,
                    },
                    ..SessionModelConfig::of(local.clone())
                }),
            },
        ];
        let application = f.manager.configuration_application(&id);
        let model = runtime.model_view();
        for call in &entry_points {
            // The shape is a valid request: only the literal is refused.
            serde_json::from_str::<Request>(&raw_request(call.clone(), "0.5")).unwrap();
            for literal in [
                "0.12345678901234567890",
                "-1.234567890123456789",
                "9007199254740993",
                "-9007199254740993",
                "1152921504606846976",
            ] {
                let response = connection.handle_json(&raw_request(call.clone(), literal)).await.unwrap();
                let text = serde_json::to_string(&response).unwrap();
                let Response::Failure(Failure { id, error, .. }) = response else { panic!("{text}") };
                assert_eq!((id, error.code, error.message.as_str(), error.data), (Some(RequestId::Integer(456)), -32602, "Invalid params", None));
                assert!(!text.contains("SECRET") && !text.contains(literal.trim_start_matches('-')), "{text}");
            }
            // A literal beyond binary64 is no number `serde_json` can hold at
            // all: the frame is refused before any decoding.
            let response = connection.handle_json(&raw_request(call.clone(), "1e400")).await.unwrap();
            let Response::Failure(Failure { id: None, error, .. }) = response else { panic!("1e400 decoded") };
            assert_eq!(error.code, -32700);
        }
        // Nothing was written, published, adopted or sent.
        assert_eq!(std::fs::read(&path).unwrap(), original);
        assert_eq!(f.manager.source_settings(&user).await.unwrap().user.revision, source.user.revision);
        assert!(!path.parent().unwrap().join("agents/reviewer.toml").exists());
        assert_eq!(f.manager.configuration_application(&id), application);
        assert_eq!(runtime.model_view(), model);
        assert!(f.provider.request_bodies().is_empty());

        // Exact literals, in any spelling, mean exactly what they spell at
        // every hop. A numeric-looking string stays a string.
        let exact = r#"{"a":0.1,"b":1.5,"c":1.50,"d":15e-1,"e":9007199254740992,"f":-9007199254740991,"g":1e20,"h":1e300,"n":{"list":[0.1,{"x":1e300}]},"s":"0.12345678901234567890"}"#;
        let expected: serde_json::Value = serde_json::from_str(exact).unwrap();
        assert_eq!(expected["c"], json!(1.5));
        assert_eq!(expected["d"], json!(1.5));
        assert_eq!(expected["s"], json!("0.12345678901234567890"));
        let placeholder = AuthoredRequestParams(serde_json::from_value(json!({"exact": "LITERAL"})).unwrap());
        let response = connection
            .handle_json(&raw_request(write(ConfigMutation::Model { id: "local/a".into(), authored: Some(profiled(placeholder)) }), exact))
            .await
            .unwrap();
        let Response::Success(_) = response else { panic!("{response:?}") };
        let expected = json!({"exact": expected});
        let toml = std::fs::read_to_string(&path).unwrap().parse::<toml::Table>().unwrap();
        let native = toml["models"]["local/a"]["profiles"]["p"]["request_params"].as_str().unwrap();
        assert_eq!(serde_json::Value::Object(crate::toml_authoring::parse_request_params_json(native).unwrap()), expected);
        let MethodResult::SourceSettings { projection: settings } = super::super::protocol::call(&connection, 457, Method::SourcesRead { target: user.clone() }).await else {
            panic!("sources")
        };
        let settings=settings.inline.expect("bounded fixture source");
        let projected = &settings.user.authored.as_ref().unwrap().models.as_ref().unwrap()["local/a"];
        assert_eq!(serde_json::Value::Object(projected.profiles.as_ref().unwrap()[&ModelProfileId::new("p")].request_params.0.clone()), expected);
        let candidate = settled(&f, 0).await.candidate.expect("the selected Model changed");
        f.manager.adopt_configuration(&id, &candidate.identity, candidate.expected_binding).unwrap();

        // A Session override from raw text reaches the provider wire exactly.
        let override_text = r#"{"o1":1.50,"o2":-9007199254740991,"o3":15e-1,"o4":[1e20,{"o5":0.1}]}"#;
        let call = Method::ModelSet {
            target: target.clone(),
            config: Box::new(SessionModelConfig {
                profile: Some(ModelProfileId::new("p")),
                request_params: serde_json::from_value(json!({"override": "LITERAL"})).unwrap(),
                ..SessionModelConfig::of(local.clone())
            }),
        };
        let Response::Success(_) = connection.handle_json(&raw_request(call, override_text)).await.unwrap() else {
            panic!("exact override refused")
        };
        let mut effective = expected.as_object().unwrap().clone();
        effective.insert("override".into(), serde_json::from_str(override_text).unwrap());
        assert_eq!(runtime.model_view().effective.as_ref().unwrap().request_params, effective);
        let mut settlement = SettlementReceiptWait::capture(&runtime);
        runtime.submit_inbound(input("exact literals")).unwrap();
        f.gates[0].wait_entered().await;
        f.gates[0].release();
        settlement.wait().await;
        let body: serde_json::Value = serde_json::from_str(&f.provider.request_body(0)).unwrap();
        for (key, value) in &effective {
            assert_eq!(&body[key], value, "{key}");
        }
        f.close().await;
    }))
    .await;
}

/// Issue #456: a Model Profile owns every top-level key it declares, and an
/// override declaring one is refused at every selection — a Session selection
/// and its explicit Summary, the root Agent and its explicit Summary, and a
/// named Agent — whatever the value's type, without the key ever reaching an
/// App Server payload: a provider-native key is as opaque as a value. Nothing
/// invalid is published or adopted and no provider request starts.
#[allow(clippy::too_many_lines)] // every selection surface of one invariant
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue456_profile_key_collisions_are_refused_without_echoing_the_key() {
    use crate::app_server::{connection::AppServerConnection, protocol::*};
    use crate::local_runtime::authoring::{ModelLayer, SummaryAuthoring};
    use crate::local_runtime::config::AgentProfileDocument;
    use crate::local_runtime::configuration::settings::SourceTarget;
    use crate::model::authoring::Profile;
    use crate::model::catalog::{ModelProfileId, ModelRef};
    use crate::model::session::{SessionModelConfig, SummaryModelPolicy};
    use crate::toml_authoring::AuthoredRequestParams;
    use serde_json::json;
    const MARKER: &str = "SECRET_MARKER";
    const KEY: &str = "sk-SECRET_MARKER";
    // The authoring projection returns each document's own authored data,
    // key included; no diagnostic it publishes about it repeats the key.
    async fn sources_quiet(connection: &AppServerConnection) {
        let MethodResult::SourceSettings { projection } = super::super::protocol::call(
            connection,
            3,
            Method::SourcesRead {
                target: SourceTarget::User,
            },
        )
        .await
        else {
            panic!("sources")
        };
        let projection = projection.inline.expect("bounded fixture source");
        assert!(
            !serde_json::to_string(&projection.application)
                .unwrap()
                .contains(MARKER)
        );
        assert!(
            !projection
                .prospective_diagnostic
                .unwrap_or_default()
                .contains(MARKER)
        );
        assert!(
            !projection
                .user
                .diagnostic
                .unwrap_or_default()
                .contains(MARKER)
        );
    }
    let object = |value: serde_json::Value| -> crate::model::invocation::RequestParams {
        serde_json::from_value(value).unwrap()
    };
    Box::pin(bounded(async {
        let f = Fixture::with_session_count(None, 1).await;
        let connection = AppServerConnection::new(f.host.clone());
        super::super::protocol::initialize(&connection).await;
        let target = super::super::protocol::attach(&connection, &f, 0).await;
        let id = f.sessions[0].id.clone();
        let runtime = f.manager.configuration_runtime(&id).unwrap();
        let local = ModelRef::parse("local/a").unwrap();
        let p = ModelProfileId::new("p");
        let user = SourceTarget::User;
        let mut model = f.manager.source_settings(&user).await.unwrap().user.authored.unwrap().models.unwrap()
            ["local/a"]
            .clone();
        model.request_params = None;
        model.default_profile = Some(p.clone());
        model.profiles = Some(
            [(
                p.clone(),
                Profile {
                    reasoning_enabled: None,
                    max_output_tokens: None,
                    request_params: AuthoredRequestParams(object(json!({KEY: null, "temperature": 0.5}))),
                },
            )]
            .into(),
        );
        write(&f, 0, ConfigMutation::Model { id: "local/a".into(), authored: Some(model) }).await;
        let candidate = settled(&f, 0).await.candidate.expect("the selected Model changed");
        f.manager.adopt_configuration(&id, &candidate.identity, candidate.expected_binding).unwrap();
        let resources = runtime.runtime_resources();
        let view = runtime.model_view();
        assert_eq!(view.effective.as_ref().unwrap().request_params, object(json!({KEY: null, "temperature": 0.5})));
        let quiet = |text: &str| assert!(!text.contains(MARKER), "{text}");

        // Session selections: the key collides whatever its value's type; an
        // explicit Summary is its own override layer. The refusal names the
        // layer and the Profile, never the key.
        let selection = |params: serde_json::Value| SessionModelConfig {
            request_params: object(params),
            ..SessionModelConfig::of(local.clone())
        };
        for (config, layer) in [
            (selection(json!({"top_k": 40, KEY: 1})), "session request-parameter overrides"),
            (selection(json!({KEY: null})), "session request-parameter overrides"),
            (selection(json!({KEY: {"nested": [KEY]}})), "session request-parameter overrides"),
            (selection(json!({"temperature": false})), "session request-parameter overrides"),
            (
                SessionModelConfig {
                    summary_model: SummaryModelPolicy::Explicit {
                        model: local.clone(),
                        profile: Some(p.clone()),
                        request_params: object(json!({KEY: "x"})),
                        max_output_tokens: None,
                    },
                    ..SessionModelConfig::of(local.clone())
                },
                "explicit summary request-parameter overrides",
            ),
        ] {
            let response = connection
                .handle_request(Request {
                    jsonrpc: JsonRpcVersion::V2,
                    id: RequestId::Integer(1),
                    call: Method::ModelSet { target: target.clone(), config: Box::new(config) },
                })
                .await;
            let text = serde_json::to_string(&response).unwrap();
            quiet(&text);
            let Response::Failure(Failure {
                error: RpcError { data: Some(ErrorData::ModelUnavailable { diagnostic }), .. },
                ..
            }) = response
            else {
                panic!("{text}")
            };
            assert!(
                diagnostic.contains(&format!(
                    "Model Profile parameter ownership collision: the {layer} declare a top-level key profile \"p\" already owns"
                )),
                "{diagnostic}"
            );
            assert_eq!(runtime.model_view(), view);
        }
        // An unrelated override key is no collision.
        assert!(matches!(super::super::protocol::call(
            &connection, 2,
            Method::ModelSet { target: target.clone(), config: Box::new(selection(json!({"top_k": 40}))) },
        ).await, MethodResult::ModelSet { .. }));
        let model = super::super::protocol::read_model_result(
            &connection, &target,
            super::super::protocol::call(&connection, 10_002, Method::ModelGet { target: target.clone() }).await,
        ).await;
        assert_eq!(model.effective.as_ref().unwrap().request_params, object(json!({KEY: null, "temperature": 0.5, "top_k": 40})));
        let view = runtime.model_view();
        let resources_after_selection = runtime.runtime_resources();
        assert_eq!(resources_after_selection.revision(), resources.revision());

        // Authored selections: the root Agent, its explicit Summary and a
        // named Agent the root admits. The source is a well-formed document;
        // its effective configuration is refused, so nothing is published.
        let root = |request_params: serde_json::Value, summary: Option<serde_json::Value>| ModelLayer {
            model: Some(local.clone()),
            profile: Some(p.clone()),
            request_params: Some(AuthoredRequestParams(object(request_params))),
            max_output_tokens: None,
            summary_model: summary.map(|params| SummaryAuthoring::Explicit {
                model: local.clone(),
                profile: Some(p.clone()),
                request_params: AuthoredRequestParams(object(params)),
                max_output_tokens: None,
            }),
        };
        for (layer, expected) in [
            (root(json!({KEY: 0}), None), "session request-parameter overrides"),
            (root(json!({}), Some(json!({"temperature": [KEY]}))), "explicit summary request-parameter overrides"),
        ] {
            write(&f, 0, ConfigMutation::RootModel { authored: Some(layer) }).await;
            let application = settled(&f, 0).await;
            let text = serde_json::to_string(&application).unwrap();
            quiet(&text);
            assert!(application.candidate.is_none(), "{text}");
            assert!(text.contains("Model Profile parameter ownership collision"), "{text}");
            assert!(text.contains(expected), "{text}");
            sources_quiet(&connection).await;
            assert!(Arc::ptr_eq(&resources_after_selection, &runtime.runtime_resources()));
            assert_eq!(runtime.model_view(), view);
        }
        write(&f, 0, ConfigMutation::RootModel { authored: Some(root(json!({}), None)) }).await;
        settled(&f, 0).await;
        // A valid default edit publishes the Provider unit to this Session
        // without re-resolving its own invocation or touching capabilities.
        let resources_after_default = runtime.runtime_resources();
        assert_eq!(resources_after_default.revision(), resources_after_selection.revision().next());
        assert!(Arc::ptr_eq(resources_after_default.capability(), resources_after_selection.capability()));
        assert_eq!(runtime.model_view(), view);

        let reviewer = crate::runtime::subagent::SubagentName::parse("reviewer").unwrap();
        let source = f.manager.source_settings(&user).await.unwrap();
        f.manager
            .write_source_settings(&user, source.absent_resource_revision.clone(), SourceMutation::Agent {
                        name: reviewer.clone(),
                        authored: Some(AgentProfileDocument {
                            description: "Reviews".into(),
                            instructions: "Review.".into(),
                            model: Some(SessionModelConfig {
                                profile: Some(p.clone()),
                                request_params: object(json!({"top_k": 1, KEY: true})),
                                ..SessionModelConfig::of(local.clone())
                            }),
                            ..AgentProfileDocument::default()
                        }),
                    })
            .await
            .unwrap();
        write(&f, 0, ConfigMutation::Agents { authored: Some(vec![reviewer.clone()]) }).await;
        let application = settled(&f, 0).await;
        let text = serde_json::to_string(&application).unwrap();
        quiet(&text);
        assert!(application.candidate.is_none(), "{text}");
        let UnitApplication::Failed { diagnostic } = &application.units[&ApplyUnit::Capabilities] else {
            panic!("{text}")
        };
        assert!(
            diagnostic.ends_with(
                "model local/a: Model Profile parameter ownership collision: the session \
                 request-parameter overrides declare a top-level key profile \"p\" already owns"
            ),
            "{diagnostic}"
        );
        sources_quiet(&connection).await;
        assert!(Arc::ptr_eq(&resources_after_default, &runtime.runtime_resources()));
        assert_eq!(runtime.model_view(), view);
        assert!(f.provider.request_bodies().is_empty());
        f.close().await;
    }))
    .await;
}

/// Issue #456: following the Model default and pinning the Profile that is
/// currently the default are distinct configured intents with one effective
/// invocation. Pinning rebuilds nothing; when the catalog default moves, the
/// following Session is offered the new invocation and the pinned one is not.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue456_following_and_pinned_default_profiles_diverge_only_when_the_default_moves() {
    use crate::model::authoring::Profile;
    use crate::model::catalog::{ModelProfileId, ModelRef};
    use crate::model::session::SessionModelConfig;
    use crate::toml_authoring::AuthoredRequestParams;
    use serde_json::json;
    Box::pin(bounded(async {
        let f = Fixture::new().await;
        let (following, pinned) = (f.sessions[0].id.clone(), f.sessions[1].id.clone());
        for id in [&following, &pinned] {
            f.manager.load(id, None).await.unwrap();
        }
        let authored = f
            .manager
            .source_settings(&crate::local_runtime::configuration::settings::SourceTarget::User)
            .await
            .unwrap()
            .user
            .authored
            .unwrap()
            .models
            .unwrap()["local/a"]
            .clone();
        let model = |default: &str| {
            let mut model = authored.clone();
            model.request_params = None;
            model.default_profile = Some(ModelProfileId::new(default));
            model.profiles = Some(
                [("balanced", 0.5), ("fast", 1.0)]
                    .into_iter()
                    .map(|(name, temperature)| {
                        (
                            ModelProfileId::new(name),
                            Profile {
                                reasoning_enabled: None,
                                max_output_tokens: None,
                                request_params: AuthoredRequestParams(
                                    serde_json::from_value(json!({"temperature": temperature}))
                                        .unwrap(),
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
        write(&f, 0, model("balanced")).await;
        for (index, id) in [(0, &following), (1, &pinned)] {
            let candidate = settled(&f, index)
                .await
                .candidate
                .expect("the selected Model changed");
            f.manager
                .adopt_configuration(id, &candidate.identity, candidate.expected_binding)
                .unwrap();
        }
        let runtime = |id| f.manager.configuration_runtime(id).unwrap();
        // Pinning the current default records the intent and rebuilds nothing.
        let pinned_resources = runtime(&pinned).runtime_resources();
        let explicit = SessionModelConfig {
            profile: Some(ModelProfileId::new("balanced")),
            ..SessionModelConfig::of(ModelRef::parse("local/a").unwrap())
        };
        let adopted = f
            .manager
            .set_model(&pinned, explicit.clone())
            .await
            .unwrap()
            .0;
        assert_eq!(adopted.configured, explicit);
        assert_eq!(
            adopted.effective,
            runtime(&following).model_view().effective
        );
        assert_eq!(runtime(&following).model_view().configured.profile, None);
        assert_eq!(
            pinned_resources.revision(),
            runtime(&pinned).runtime_resources().revision()
        );
        let pinned_resources = runtime(&pinned).runtime_resources();
        let pinned_invocation = runtime(&pinned).model_snapshot();

        // The default moves: only the following Session's invocation changes.
        let following_resources = runtime(&following).runtime_resources();
        write(&f, 0, model("fast")).await;
        let candidate = settled(&f, 0)
            .await
            .candidate
            .expect("the followed default moved");
        assert!(
            settled(&f, 1).await.candidate.is_none(),
            "the pinned Profile did not move"
        );
        // The pinned Session only receives the published catalog.
        let published = runtime(&pinned).runtime_resources();
        assert_eq!(published.revision(), pinned_resources.revision().next());
        assert!(Arc::ptr_eq(
            pinned_resources.capability(),
            published.capability()
        ));
        assert_eq!(runtime(&pinned).model_snapshot(), pinned_invocation);
        assert_eq!(
            runtime(&pinned).model_catalog().models[0]
                .default_profile
                .as_ref()
                .map(ToString::to_string)
                .as_deref(),
            Some("fast")
        );
        assert!(Arc::ptr_eq(
            &following_resources,
            &runtime(&following).runtime_resources()
        ));
        f.manager
            .adopt_configuration(&following, &candidate.identity, candidate.expected_binding)
            .unwrap();
        let (follows, pins) = (
            runtime(&following).model_view(),
            runtime(&pinned).model_view(),
        );
        assert_eq!(
            (
                follows.configured.profile,
                follows.effective.as_ref().unwrap().profile.clone()
            ),
            (None, Some(ModelProfileId::new("fast")))
        );
        assert_eq!(pins.configured, explicit);
        assert_eq!(
            pins.effective.as_ref().unwrap().profile,
            Some(ModelProfileId::new("balanced"))
        );
        assert_eq!(
            follows.effective.as_ref().unwrap().request_params["temperature"],
            json!(1.0)
        );
        assert_eq!(
            pins.effective.as_ref().unwrap().request_params["temperature"],
            json!(0.5)
        );
        f.close().await;
    }))
    .await;
}
