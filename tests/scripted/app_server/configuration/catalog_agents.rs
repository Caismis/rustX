//! Agent definitions in catalog publications: inherited agents, frozen
//! admitted children, Agent changes committed with their capability closure,
//! and the agent control group of one admitted generation.
#![allow(clippy::large_futures)] // bounded fixture futures; no recursive or unbounded stack growth
use super::*;

/// Issue #459: desired Named Agent membership is not an adopted invocation.
/// One valid source generation removes the selected `local/a`, adds `local/c`
/// as the new default and admits `reviewer`, an Agent that names no model.
/// With Capability preparation held, the catalog is already published: both
/// native surfaces discover `local/c`, `local/a` is reported unavailable, and
/// the committed closure — no `reviewer`, no `subagent` or `review` Tool — is
/// the same snapshot. Released, the Capability closure commits automatically
/// while `local/a` stays explicitly unavailable; `session/setModel(local/c)`
/// reopens admission, and the next Attempt and the Workflow child it drives
/// run `local/c` with `reviewer`.
#[allow(clippy::too_many_lines)] // one ordered acceptance sequence
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue459_new_inherited_agent_does_not_hold_back_catalog_publication() {
    use super::super::protocol::{attach, call, initialize};
    use crate::app_server::connection::AppServerConnection;
    use crate::app_server::protocol::Method;
    use crate::model::catalog::ModelRef;
    use crate::model::session::SessionModelConfig;
    use crate::runtime::conversation_runtime::InboundAdmissionError;
    use serde_json::json;
    Box::pin(bounded(async {
        let fixture = Fixture::with_session_count(Some("review"), 1).await;
        let id = fixture.sessions[0].id.clone();
        issue459_reviewer_sources(&fixture);
        issue459_global_review(&fixture);
        let connection = AppServerConnection::new(fixture.host.clone());
        initialize(&connection).await;
        let target = attach(&connection, &fixture, 0).await;
        fixture.manager.load(&id, None).await.unwrap();
        let runtime = fixture.manager.configuration_runtime(&id).unwrap();
        let host = fixture.manager.configuration_host(&id).unwrap();
        // Read-only beside the App Server's writer attachment.
        let (client, _) = host
            .attach_read_only(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
            .unwrap();
        let model = |id: &str| SessionModelConfig::of(ModelRef::parse(id).unwrap());
        // The fresh Session already selects `local/a`.
        assert_eq!(runtime.model_view().configured, model("local/a"));
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
        assert!(old.config.agent.agents.is_empty() && old.config.agent.workflows.is_empty());
        let selected = runtime.model_view();
        let binding =
            crate::local_runtime::configuration::settings::EffectiveConfiguration::adopted(
                &runtime,
            )
            .unwrap()
            .adopted_binding;
        let probe = fixture
            .manager
            .probe(&fixture.sessions[0].active_conversation_id);

        probe.before_configuration_prepare.arm();
        issue459_edit(&fixture, |document| {
            issue459_replace_a_with_c(document);
            issue459_admit_reviewer(document);
        })
        .await;
        probe.before_configuration_prepare.entered().await;

        // Capability preparation is held; the catalog is already published.
        let application = fixture.manager.configuration_application(&id).unwrap();
        let published_revision = application.desired.input_revision.clone().unwrap();
        assert_eq!(
            application.units[&ApplyUnit::Provider],
            UnitApplication::Applied,
            "{application:?}"
        );
        assert_eq!(
            application.units[&ApplyUnit::Capabilities],
            UnitApplication::Preparing,
            "{application:?}"
        );
        assert!(
            application
                .units
                .values()
                .all(|unit| *unit != UnitApplication::Ready)
        );
        assert_eq!(issue459_model_refs(&runtime), ["local/b", "local/c"]);
        let catalog =
            super::super::protocol::read_native_catalog_result(client.model_catalog().unwrap());
        assert_eq!(catalog, runtime.model_catalog());
        let served = super::super::protocol::read_catalog_result(
            call(
                &connection,
                100,
                Method::ModelCatalog {
                    target: target.clone(),
                },
            )
            .await,
        );
        assert_eq!(served, catalog);
        // `local/a` stays configured, reported unavailable, nothing substituted.
        let view = runtime.model_view();
        assert_eq!(view.configured, model("local/a"));
        assert!(view.unavailable.as_deref().unwrap().contains("local/a"));
        assert_eq!(view.effective, selected.effective);
        // The adopted closure is untouched: `reviewer` is not admitted.
        let published = runtime.runtime_resources();
        assert!(Arc::ptr_eq(before.capability(), published.capability()));
        let new = published.configuration().unwrap().clone();
        assert!(new.config.agent.agents.is_empty() && new.config.agent.workflows.is_empty());
        assert_eq!(new.authored.source_revisions, old.authored.source_revisions);
        assert_eq!(
            new.authored.component_revisions[&ApplyUnit::Provider],
            published_revision
        );
        assert_eq!(
            new.authored
                .component_revisions
                .get(&ApplyUnit::Capabilities),
            old.authored
                .component_revisions
                .get(&ApplyUnit::Capabilities)
        );
        assert_eq!(
            crate::local_runtime::configuration::settings::EffectiveConfiguration::adopted(
                &runtime
            )
            .unwrap()
            .adopted_binding,
            binding + 1
        );
        let binding_now = retained();
        assert_eq!(binding_now.binding_revision, binding + 1);
        assert!(binding_now.config().agent.agents.is_empty());
        assert_eq!(binding_now.source_revisions, old.authored.source_revisions);
        assert!(matches!(
            runtime.submit_inbound(issue459_text("refused")),
            Err(InboundAdmissionError::ModelUnavailable { .. })
        ));

        // The Capability change prepares over the published binding and the
        // idle Session commits it at once: no client confirms it, and the
        // unavailable selection stays explicit rather than substituted.
        probe.before_configuration_prepare.release();
        let application = applied(&fixture, 0).await;
        assert_eq!(
            application.units[&ApplyUnit::Provider],
            UnitApplication::Applied
        );
        assert_eq!(
            application.units[&ApplyUnit::Capabilities],
            UnitApplication::Applied,
            "{application:?}"
        );
        let committed = runtime.runtime_resources().configuration().unwrap().clone();
        assert_eq!(
            committed
                .config
                .agent
                .agents
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
            ["reviewer"]
        );
        assert_eq!(runtime.model_view().configured, model("local/a"));
        assert!(runtime.model_view().unavailable.is_some());
        assert!(fixture.provider.request_bodies().is_empty());

        // Explicit recovery through `session/setModel` reopens admission.
        assert!(matches!(
            call(
                &connection,
                101,
                Method::ModelSet {
                    recover_unavailable_auxiliary: None,
                    target: target.clone(),
                    config: Box::new(model("local/c"))
                },
            )
            .await,
            crate::app_server::protocol::MethodResult::ModelSet { .. }
        ));
        let view = super::super::protocol::read_model_result(
            &connection,
            &target,
            call(
                &connection,
                10_101,
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
        applied(&fixture, 0).await;
        assert_eq!(runtime.model_view(), *view);
        assert_eq!(retained().session_model(), &model("local/c"));

        // The next Attempts run `local/c` with the committed `reviewer`, and
        // the Workflow child they drive inherits `local/c` at admission.
        let mut settlement = SettlementReceiptWait::capture(&runtime);
        runtime.submit_inbound(issue459_text("on c")).unwrap();
        fixture.gates[0].wait_entered().await;
        fixture.gates[0].release();
        settlement.wait().await;
        let mut settlement = SettlementReceiptWait::capture(&runtime);
        runtime
            .submit_inbound(issue459_text("after selection"))
            .unwrap();
        settlement.wait().await;
        let bodies = fixture
            .provider
            .request_bodies()
            .iter()
            .map(|body| serde_json::from_str::<serde_json::Value>(body).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            (
                &bodies[0]["model"],
                &bodies[0]["temperature"],
                &bodies[0]["seed"]
            ),
            (&json!("c"), &json!(0.25), &json!(7))
        );
        let tools = issue459_tool_names(&bodies[0]);
        assert!(
            tools.iter().any(|tool| tool == "review")
                && tools.iter().any(|tool| tool == "subagent"),
            "{tools:?}"
        );
        let child = bodies
            .iter()
            .find(|body| {
                issue459_tool_names(body)
                    .iter()
                    .any(|tool| tool == "workflow_output")
            })
            .expect("reviewer child request");
        assert_eq!(
            (&child["model"], &child["temperature"], &child["seed"]),
            (&json!("c"), &json!(0.25), &json!(7))
        );
        assert!(child.to_string().contains("issue459 reviewer"));
        assert!(bodies.iter().all(|body| body["model"] == json!("c")));
        drop(client);
        fixture.close().await;
    }))
    .await;
}

/// The request bodies sent from `from` on, parsed.
fn issue459_bodies_from(fixture: &Fixture, from: usize) -> Vec<serde_json::Value> {
    fixture.provider.request_bodies()[from..]
        .iter()
        .map(|body| serde_json::from_str(body).unwrap())
        .collect()
}

/// The one Workflow child request among `bodies`.
fn issue459_child(bodies: &[serde_json::Value]) -> &serde_json::Value {
    let children: Vec<_> = bodies
        .iter()
        .filter(|body| {
            issue459_tool_names(body)
                .iter()
                .any(|tool| tool == "workflow_output")
        })
        .collect();
    assert_eq!(children.len(), 1, "{bodies:?}");
    children[0]
}

/// Issue #459 with an adopted Named Agent that names its own model.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Issue459AgentEdit {
    /// `reviewer`'s instructions change; it still names `local/b`.
    Definition,
    /// `reviewer` now names the new `local/c` instead of `local/b`.
    Selection,
    /// The catalog entry `local/b` that `reviewer` names gains parameters.
    CatalogEntry,
}

/// Root runs the Workflow `review`, whose Agent `reviewer` names `local/b`.
/// One generation removes the Session's `local/a`, adds `local/c` and makes
/// `edit`. A changed definition or selection is the Capability unit's and a
/// changed catalog entry changes the committed invocation itself; either way
/// the idle Session commits the complete closure automatically, while the
/// removed Session selection stays explicitly unavailable. The explicit
/// selection of `local/c` reopens admission, and the next child runs exactly
/// the committed definition and invocation.
#[allow(clippy::too_many_lines)] // one ordered sequence per edit
async fn issue459_explicit_agent_edit(edit: Issue459AgentEdit) {
    use crate::model::catalog::ModelRef;
    use crate::model::session::SessionModelConfig;
    use serde_json::json;
    let fixture = Fixture::with_tool(Some("review")).await;
    let id = fixture.sessions[0].id.clone();
    issue459_reviewer_sources(&fixture);
    issue459_write_reviewer(&fixture, "issue459 reviewer", Some("local/b"));
    std::fs::write(
        fixture.workspaces[0].join("rustx.toml"),
        "[agent]\nworkflows=['review']\n",
    )
    .unwrap();
    fixture.manager.load(&id, None).await.unwrap();
    let runtime = fixture.manager.configuration_runtime(&id).unwrap();
    let model = |id: &str| SessionModelConfig::of(ModelRef::parse(id).unwrap());
    let before = runtime.runtime_resources();
    fixture.gates[0].release();
    let attempt = |text: &'static str| {
        let runtime = runtime.clone();
        let fixture = &fixture;
        let from = fixture.provider.request_bodies().len();
        async move {
            let mut settlement = SettlementReceiptWait::capture(&runtime);
            runtime.submit_inbound(issue459_text(text)).unwrap();
            settlement.wait().await;
            issue459_bodies_from(fixture, from)
        }
    };

    match edit {
        Issue459AgentEdit::Definition => {
            issue459_write_reviewer(&fixture, "issue459 reviewer v2", Some("local/b"));
        }
        Issue459AgentEdit::Selection => {
            issue459_write_reviewer(&fixture, "issue459 reviewer", Some("local/c"));
        }
        Issue459AgentEdit::CatalogEntry => {}
    }
    issue459_edit(&fixture, |document| {
        issue459_replace_a_with_c(document);
        if edit == Issue459AgentEdit::CatalogEntry {
            document["models"]["local/b"]
                .as_table_mut()
                .unwrap()
                .insert("request_params".into(), "{\"temperature\":0.75}".into());
        }
    })
    .await;
    let application = applied(&fixture, 0).await;
    assert!(
        application
            .units
            .values()
            .all(|unit| *unit == UnitApplication::Applied),
        "{application:?}"
    );
    assert!(!Arc::ptr_eq(
        before.capability(),
        runtime.runtime_resources().capability()
    ));
    assert_eq!(issue459_model_refs(&runtime), ["local/b", "local/c"]);
    let view = runtime.model_view();
    assert_eq!(view.configured, model("local/a"));
    assert!(view.unavailable.as_deref().unwrap().contains("local/a"));
    assert!(fixture.provider.request_bodies().is_empty());

    fixture
        .manager
        .set_model(&id, model("local/c"))
        .await
        .unwrap();
    applied(&fixture, 0).await;
    assert_eq!(runtime.model_view().configured, model("local/c"));
    let bodies = attempt("after selection").await;
    assert!(bodies.iter().any(|body| body["model"] == json!("c")));
    let child = issue459_child(&bodies);
    match edit {
        Issue459AgentEdit::Definition => {
            assert_eq!(child["model"], json!("b"));
            assert!(child.to_string().contains("issue459 reviewer v2"));
        }
        Issue459AgentEdit::Selection => assert_eq!(
            (&child["model"], &child["temperature"], &child["seed"]),
            (&json!("c"), &json!(0.25), &json!(7))
        ),
        Issue459AgentEdit::CatalogEntry => assert_eq!(
            (&child["model"], &child["temperature"]),
            (&json!("b"), &json!(0.75))
        ),
    }
    fixture.close().await;
}

/// Issue #459: an Attempt and the child it admitted stay frozen across a
/// mixed publication. Root runs the Workflow `review`, whose `reviewer`
/// inherits the Attempt's `local/a`; the child is parked at its first
/// provider response (Root's own requests carry `request-B` and pass). One
/// generation then removes `local/a`, adds `local/c`, admits `reviewer` by
/// name and rewrites its instructions. The catalog publishes, but the parked
/// child finishes with `local/a` and the instructions it froze, and Root's
/// continuation keeps `local/a` and its Tools: no `subagent` leaks in. The
/// publication changes only future work: the closure commits after the
/// Attempt settles, new work is refused until `local/c` is selected, and the
/// next child then inherits `local/c` and the committed definition.
#[allow(clippy::too_many_lines)] // one ordered acceptance sequence
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue459_admitted_child_stays_frozen_across_mixed_agent_publication() {
    use crate::model::catalog::ModelRef;
    use crate::model::session::SessionModelConfig;
    use crate::runtime::conversation_runtime::InboundAdmissionError;
    use serde_json::json;
    Box::pin(bounded(async {
        let fixture = Fixture::with_tool(Some("review")).await;
        let id = fixture.sessions[0].id.clone();
        issue459_reviewer_sources(&fixture);
        std::fs::write(
            fixture.workspaces[0].join("rustx.toml"),
            "[agent]\nworkflows=['review']\n",
        )
        .unwrap();
        fixture.manager.load(&id, None).await.unwrap();
        let runtime = fixture.manager.configuration_runtime(&id).unwrap();
        let model = |id: &str| SessionModelConfig::of(ModelRef::parse(id).unwrap());
        let before = runtime.runtime_resources();

        fixture.gates[1].release();
        let mut settlement = SettlementReceiptWait::capture(&runtime);
        runtime
            .submit_inbound(issue459_text("request-B frozen"))
            .unwrap();
        // The child is admitted and parked at its first provider response.
        fixture.gates[0].wait_entered().await;

        issue459_write_reviewer(&fixture, "issue459 reviewer v2", None);
        issue459_edit(&fixture, |document| {
            issue459_replace_a_with_c(document);
            document["agent"]
                .as_table_mut()
                .unwrap()
                .insert("agents".into(), vec![toml::Value::from("reviewer")].into());
        })
        .await;
        let application = settled(&fixture, 0).await;
        assert_eq!(
            application.units[&ApplyUnit::Provider],
            UnitApplication::Applied,
            "{application:?}"
        );
        assert_eq!(
            application.units[&ApplyUnit::Capabilities],
            UnitApplication::Ready,
            "the Agent change waits for the admitted Attempt"
        );
        assert_eq!(issue459_model_refs(&runtime), ["local/b", "local/c"]);
        assert!(runtime.model_view().unavailable.is_some());
        assert!(Arc::ptr_eq(
            before.capability(),
            runtime.runtime_resources().capability()
        ));

        fixture.gates[0].release();
        settlement.wait().await;
        let bodies = issue459_bodies_from(&fixture, 0);
        assert!(
            bodies.iter().all(|body| body["model"] == json!("a")),
            "{bodies:?}"
        );
        let child = issue459_child(&bodies);
        assert!(child.to_string().contains("issue459 reviewer"));
        assert!(!child.to_string().contains("issue459 reviewer v2"));
        let roots: Vec<_> = bodies
            .iter()
            .filter(|body| body.to_string().contains("request-B frozen"))
            .collect();
        assert_eq!(roots.len(), 2, "proposal and continuation");
        for root in roots {
            let tools = issue459_tool_names(root);
            assert!(tools.contains(&"review".to_owned()), "{tools:?}");
            assert!(!tools.contains(&"subagent".to_owned()), "{tools:?}");
        }

        // Settlement committed the staged closure; new work still waits for
        // an explicit correction of the removed selection.
        applied(&fixture, 0).await;
        assert!(matches!(
            runtime.submit_inbound(issue459_text("request-B refused")),
            Err(InboundAdmissionError::ModelUnavailable { .. })
        ));
        fixture
            .manager
            .set_model(&id, model("local/c"))
            .await
            .unwrap();
        applied(&fixture, 0).await;
        let from = fixture.provider.request_bodies().len();
        let mut settlement = SettlementReceiptWait::capture(&runtime);
        runtime
            .submit_inbound(issue459_text("request-B on c"))
            .unwrap();
        settlement.wait().await;
        let bodies = issue459_bodies_from(&fixture, from);
        assert!(
            bodies.iter().all(|body| body["model"] == json!("c")),
            "{bodies:?}"
        );
        let child = issue459_child(&bodies);
        assert_eq!(
            (&child["temperature"], &child["seed"]),
            (&json!(0.25), &json!(7))
        );
        // The committed closure: the new definition and `subagent`.
        assert!(child.to_string().contains("issue459 reviewer v2"));
        assert!(
            bodies
                .iter()
                .any(|body| issue459_tool_names(body).contains(&"subagent".to_owned()))
        );
        fixture.close().await;
    }))
    .await;
}

/// Issue #459: a committed Agent's changed definition commits with its unit.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue459_agent_definition_change_commits_with_its_capability_closure() {
    Box::pin(bounded(issue459_explicit_agent_edit(
        Issue459AgentEdit::Definition,
    )))
    .await;
}

/// Issue #459: a committed Agent's changed explicit model selection commits
/// with its unit.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue459_agent_selection_change_commits_with_its_capability_closure() {
    Box::pin(bounded(issue459_explicit_agent_edit(
        Issue459AgentEdit::Selection,
    )))
    .await;
}

/// Issue #459: a catalog that changes what a committed Agent's own selection
/// resolves to commits with the complete closure.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue459_catalog_changing_an_agent_invocation_commits_its_closure() {
    Box::pin(bounded(issue459_explicit_agent_edit(
        Issue459AgentEdit::CatalogEntry,
    )))
    .await;
}

/// The Agent control names and the `subagent` routing description of every
/// provider request body after `from`.
fn issue462_offered_controls(fixture: &Fixture, from: usize) -> Vec<(Vec<String>, String)> {
    fixture.provider.request_bodies()[from..]
        .iter()
        .map(|body| {
            let request: serde_json::Value = serde_json::from_str(body).unwrap();
            let tools = request["tools"].as_array().cloned().unwrap_or_default();
            let names = tools
                .iter()
                .filter_map(|tool| tool["function"]["name"].as_str())
                .filter(|name| crate::tools::native::AGENT_CONTROL_TOOL_NAMES.contains(name))
                .map(str::to_owned)
                .collect();
            let routing = tools
                .iter()
                .find(|tool| tool["function"]["name"] == "subagent")
                .map(|tool| tool["function"]["description"].as_str().unwrap().to_owned())
                .unwrap_or_default();
            (names, routing)
        })
        .collect()
}

async fn issue462_admit(fixture: &Fixture, agents: &[&str]) -> ConfigurationApplication {
    let agents: Vec<toml::Value> = agents.iter().map(|name| toml::Value::from(*name)).collect();
    issue459_edit(fixture, move |document| {
        document["agent"]
            .as_table_mut()
            .unwrap()
            .insert("agents".into(), agents.into());
    })
    .await;
    settled(fixture, 0).await
}

/// Issue #462 A01/A02/A08/A09/A10/A14: the five Agent controls reach the
/// provider's `tools[]` exactly when the Root admits at least one valid named
/// Agent, as one group of one admitted generation. Discovered but unselected
/// definitions compose nothing; `0 -> 5 -> 0` and a nonempty-to-nonempty
/// change publish matching names and routing atomically; an admitted Attempt
/// keeps its original group and routing across a publication; an invalid
/// selection publishes nothing and leaves the previous generation effective.
#[allow(clippy::too_many_lines)] // one ordered publication sequence
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue462_agent_control_group_follows_one_admitted_generation() {
    use crate::local_runtime::configuration::settings::SourceTarget;
    Box::pin(bounded(async {
        let fixture = Fixture::with_tool(Some("read")).await;
        let id = fixture.sessions[0].id.clone();
        let agents = fixture.workspaces[0].join(".agents/agents");
        std::fs::create_dir_all(&agents).unwrap();
        for (name, description) in [("reviewer", "Review code"), ("explorer", "Explore code")] {
            std::fs::write(
                agents.join(format!("{name}.toml")),
                format!("description='{description}'\ninstructions='Act as {name}'\n"),
            )
            .unwrap();
        }
        // The launch generation admits `reviewer` only.
        let user = fixture
            .manager
            .source_settings(&SourceTarget::User)
            .await
            .unwrap()
            .user
            .path;
        let mut document: toml::Value =
            toml::from_str(&std::fs::read_to_string(&user).unwrap()).unwrap();
        document["agent"]
            .as_table_mut()
            .unwrap()
            .insert("agents".into(), vec![toml::Value::from("reviewer")].into());
        std::fs::write(&user, toml::to_string(&document).unwrap()).unwrap();
        fixture.manager.load(&id, None).await.unwrap();
        let runtime = fixture.manager.configuration_runtime(&id).unwrap();
        let five = |names: &[String]| {
            let mut sorted = names.to_vec();
            sorted.sort();
            let mut expected = crate::tools::native::AGENT_CONTROL_TOOL_NAMES.to_vec();
            expected.sort_unstable();
            sorted == expected
        };

        // The first Attempt (the only one this fixture drives through a Tool
        // call) is held while a nonempty-to-nonempty publication lands.
        runtime.submit_inbound(input("request-B held")).unwrap();
        fixture.gates[1].wait_entered().await;
        let admitted =
            crate::local_runtime::configuration::settings::EffectiveConfiguration::adopted(
                &runtime,
            )
            .unwrap()
            .admitted_attempt
            .unwrap();
        let replacement = issue462_admit(&fixture, &["explorer"]).await;
        assert_eq!(
            crate::local_runtime::configuration::settings::EffectiveConfiguration::adopted(
                &runtime
            )
            .unwrap()
            .admitted_attempt
            .unwrap(),
            admitted
        );
        let mut settlement = SettlementReceiptWait::capture(&runtime);
        fixture.gates[1].release();
        settlement.wait().await;
        runtime.wait_for_configuration_admissions().await;
        let held = issue462_offered_controls(&fixture, 0);
        assert_eq!(held.len(), 2, "the held Attempt made a second request");
        for (names, routing) in held {
            assert!(five(&names), "exactly once each: {names:?}");
            assert!(routing.contains("- reviewer: Review code"), "{routing}");
            assert!(!routing.contains("explorer"), "{routing}");
        }
        drop(replacement);
        applied(&fixture, 0).await;
        fixture.gates[0].release();
        let attempt = |text: &'static str| {
            let runtime = runtime.clone();
            async move {
                let mut done = SettlementReceiptWait::capture(&runtime);
                runtime.submit_inbound(input(text)).unwrap();
                done.wait().await;
                runtime.wait_for_configuration_admissions().await;
            }
        };
        let from = fixture.provider.request_bodies().len();
        attempt("request-A explorer").await;
        let offered = issue462_offered_controls(&fixture, from);
        assert!(!offered.is_empty());
        for (names, routing) in offered {
            assert!(five(&names), "exactly once each: {names:?}");
            assert!(routing.contains("- explorer: Explore code"), "{routing}");
            assert!(!routing.contains("reviewer"), "{routing}");
        }

        // An invalid selection fails preparation and publishes nothing.
        let tools = runtime
            .runtime_resources()
            .capability()
            .tool_registry()
            .clone();
        let invalid = issue462_admit(&fixture, &["missing"]).await;
        assert!(
            invalid
                .units
                .values()
                .all(|unit| *unit != UnitApplication::Ready),
            "{invalid:?}"
        );
        assert!(Arc::ptr_eq(
            &tools,
            runtime.runtime_resources().capability().tool_registry()
        ));
        let from = fixture.provider.request_bodies().len();
        attempt("request-A after invalid").await;
        for (names, routing) in issue462_offered_controls(&fixture, from) {
            assert!(five(&names), "{names:?}");
            assert!(routing.contains("- explorer: Explore code"), "{routing}");
        }

        // 5 -> 0: no admission composes none of the group, although both
        // definitions are still discovered.
        issue462_admit(&fixture, &[]).await;
        applied(&fixture, 0).await;
        let from = fixture.provider.request_bodies().len();
        attempt("request-A zero").await;
        let offered = issue462_offered_controls(&fixture, from);
        assert!(!offered.is_empty());
        assert!(
            offered.iter().all(|(names, _)| names.is_empty()),
            "{offered:?}"
        );

        // 0 -> 5 again from the same discovered set.
        issue462_admit(&fixture, &["reviewer"]).await;
        applied(&fixture, 0).await;
        let from = fixture.provider.request_bodies().len();
        attempt("request-A reviewer").await;
        let offered = issue462_offered_controls(&fixture, from);
        assert!(!offered.is_empty());
        for (names, routing) in offered {
            assert!(five(&names), "exactly once each: {names:?}");
            assert!(routing.contains("- reviewer: Review code"), "{routing}");
            assert!(!routing.contains("explorer"), "{routing}");
        }
        fixture.close().await;
    }))
    .await;
}
