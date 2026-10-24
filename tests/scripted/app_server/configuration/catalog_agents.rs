//! Agent definitions in catalog publications: inherited agents, frozen
//! admitted children, adopted-agent changes that wait for capability adoption,
//! and the agent control group of one admitted generation.
#![allow(clippy::large_futures)] // bounded fixture futures; no recursive or unbounded stack growth
use super::*;

/// Issue #459: desired Named Agent membership is not an adopted invocation.
/// One valid source generation removes the selected `local/a`, adds `local/c`
/// as the new default and admits `reviewer`, an Agent that names no model.
/// With Capability preparation held, the catalog is already published: both
/// native surfaces discover `local/c`, `local/a` is reported unavailable, and
/// the adopted closure — no `reviewer`, no `subagent` or `review` Tool — is
/// the same snapshot. `session/setModel(local/c)` then succeeds before the
/// Capability adoption, fencing the older candidate; the next Attempt sends
/// `local/c` with the old Tools; the recaptured candidate adopts `reviewer`,
/// keeps `local/c`, and the Workflow child it drives inherits `local/c`.
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
        assert!(application.candidate.is_none());
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

        // The Capability change prepares over the published binding and waits.
        probe.before_configuration_prepare.release();
        let application = settled(&fixture, 0).await;
        assert_eq!(
            application.units[&ApplyUnit::Provider],
            UnitApplication::Applied
        );
        assert!(
            matches!(
                application.units[&ApplyUnit::Capabilities],
                UnitApplication::Ready { .. }
            ),
            "{application:?}"
        );
        let stale = application.candidate.clone().expect("capabilities wait");
        assert_eq!(stale.expected_binding, binding + 1);
        assert!(Arc::ptr_eq(
            before.capability(),
            runtime.runtime_resources().capability()
        ));

        // Explicit recovery through `session/setModel`, before that adoption.
        assert!(matches!(
            call(
                &connection,
                101,
                Method::ModelSet {
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
        assert!(matches!(
            fixture
                .manager
                .adopt_configuration(&id, &stale.identity, stale.expected_binding),
            Err(AdoptionError::Conflict)
        ));
        assert_eq!(runtime.model_view(), *view);
        let pending = settled(&fixture, 0).await;
        let candidate = pending.candidate.clone().expect("recaptured capabilities");
        assert_ne!(candidate.identity, stale.identity);
        assert_eq!(candidate.expected_binding, binding + 2);

        // Start a fresh eligibility stream after the recaptured preparation.
        // An older subscription can still contain that preparation's
        // Busy -> Eligible pair; it cannot acknowledge this Attempt's tail.
        runtime.wait_for_configuration_admissions().await;
        let mut eligibility = EligibilityStream::open(&fixture, &id);
        eligibility.until(AdoptionEligibility::Eligible).await;
        assert!(runtime.idle_epoch().is_ok());

        // The next Attempt runs `local/c` with the still-adopted Tools.
        // Hold its actual provider request so Busy is attributed to this
        // Attempt, not to an earlier configuration preparation.
        let mut settlement = SettlementReceiptWait::capture(&runtime);
        runtime.submit_inbound(issue459_text("on c")).unwrap();
        fixture.gates[0].wait_entered().await;
        eligibility.until(AdoptionEligibility::Busy).await;
        assert!(runtime.has_current_attempt());
        assert_eq!(
            runtime.idle_epoch(),
            Err(crate::runtime::conversation_runtime::IdleBusyReason::LifecycleOwner)
        );
        assert!(matches!(
            fixture.manager.adopt_configuration(
                &id,
                &candidate.identity,
                candidate.expected_binding
            ),
            Err(AdoptionError::Busy)
        ));
        assert_eq!(
            fixture
                .manager
                .configuration_application(&id)
                .unwrap()
                .candidate,
            Some(candidate.clone()),
            "the held Attempt retains the same prepared candidate"
        );
        fixture.gates[0].release();
        settlement.wait().await;
        assert_eq!(fixture.provider.request_bodies().len(), 1);
        let body = issue459_body(&fixture, 0);
        assert_eq!(
            (&body["model"], &body["temperature"], &body["seed"]),
            (&json!("c"), &json!(0.25), &json!(7))
        );
        let tools = issue459_tool_names(&body);
        assert!(
            !tools
                .iter()
                .any(|tool| tool == "review" || tool == "subagent"),
            "{tools:?}"
        );

        // Adoption admits `reviewer` and keeps the corrected selection.
        eligibility.until(AdoptionEligibility::Eligible).await;
        assert!(!runtime.has_current_attempt());
        assert!(runtime.idle_epoch().is_ok());
        fixture
            .manager
            .adopt_configuration(&id, &candidate.identity, candidate.expected_binding)
            .unwrap();
        assert_eq!(runtime.model_view(), *view);
        assert_eq!(retained().session_model(), &model("local/c"));
        let adopted = runtime.runtime_resources().configuration().unwrap().clone();
        assert_eq!(
            adopted
                .config
                .agent
                .agents
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
            ["reviewer"]
        );

        // The Workflow child `reviewer` drives inherits `local/c` at admission.
        let mut settlement = SettlementReceiptWait::capture(&runtime);
        runtime
            .submit_inbound(issue459_text("after adoption"))
            .unwrap();
        settlement.wait().await;
        let bodies = fixture
            .provider
            .request_bodies()
            .iter()
            .skip(1)
            .map(|body| serde_json::from_str::<serde_json::Value>(body).unwrap())
            .collect::<Vec<_>>();
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
/// `edit`. A changed definition or selection is the Capability unit's: the
/// adopted invocation, `local/b` as `local/b` resolves, is unchanged by the
/// new catalog, so the catalog publishes alone while the adopted closure
/// stays as it was. The change cannot be judged against the removed
/// selection, so its candidate waits; the explicit selection fences it, and
/// recaptured over `local/c` it preserves the Root request shape and adopts
/// at that boundary, after which the child runs the new definition. A
/// changed catalog entry
/// changes the adopted invocation itself: the catalog is not published on
/// its own, the Session keeps running on `local/a` and the child keeps the
/// parameters it resolved, and only adoption publishes both.
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
    let retained = || {
        fixture
            .manager
            .sessions
            .configuration_bindings
            .lock()
            .unwrap()[&id]
            .binding_revision
    };
    let binding = retained();
    let before = runtime.runtime_resources();
    let mut eligibility = EligibilityStream::open(&fixture, &id);
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
    let application = settled(&fixture, 0).await;
    let stale = application
        .candidate
        .clone()
        .expect("the Agent change waits");
    // Nothing the adopted closure owns has moved.
    assert!(Arc::ptr_eq(
        before.capability(),
        runtime.runtime_resources().capability()
    ));

    if edit == Issue459AgentEdit::CatalogEntry {
        assert!(
            matches!(
                application.units[&ApplyUnit::Provider],
                UnitApplication::Ready { .. }
            ),
            "{application:?}"
        );
        assert_eq!(issue459_model_refs(&runtime), ["local/a", "local/b"]);
        assert!(runtime.model_view().unavailable.is_none());
        assert_eq!(retained(), binding);
        let bodies = attempt("before adoption").await;
        assert!(bodies.iter().any(|body| body["model"] == json!("a")));
        let child = issue459_child(&bodies);
        assert_eq!(child["model"], json!("b"));
        assert_ne!(child["temperature"], json!(0.75));

        eligibility.until(AdoptionEligibility::Busy).await;
        eligibility.until(AdoptionEligibility::Eligible).await;
        fixture
            .manager
            .adopt_configuration(&id, &stale.identity, stale.expected_binding)
            .unwrap();
        assert_eq!(issue459_model_refs(&runtime), ["local/b", "local/c"]);
        let view = runtime.model_view();
        assert_eq!(view.configured, model("local/a"));
        assert!(view.unavailable.as_deref().unwrap().contains("local/a"));
        fixture
            .manager
            .set_model(&id, model("local/c"))
            .await
            .unwrap();
        let child = issue459_child(&attempt("after adoption").await).clone();
        assert_eq!(
            (&child["model"], &child["temperature"]),
            (&json!("b"), &json!(0.75))
        );
    } else {
        assert_eq!(
            application.units[&ApplyUnit::Provider],
            UnitApplication::Applied,
            "{application:?}"
        );
        assert!(
            matches!(
                application.units[&ApplyUnit::Capabilities],
                UnitApplication::Ready { .. }
            ),
            "{application:?}"
        );
        assert_eq!(issue459_model_refs(&runtime), ["local/b", "local/c"]);
        assert!(
            runtime
                .model_view()
                .unavailable
                .as_deref()
                .unwrap()
                .contains("local/a")
        );
        assert_eq!(retained(), binding + 1);

        fixture
            .manager
            .set_model(&id, model("local/c"))
            .await
            .unwrap();
        assert!(matches!(
            fixture
                .manager
                .adopt_configuration(&id, &stale.identity, stale.expected_binding),
            Err(AdoptionError::Conflict)
        ));
        // Recaptured over `local/c`, the Agent change no longer touches the
        // Root request shape, so it adopts at its own boundary: the older
        // closure is retired for a new one, and nothing else moved.
        let recaptured = settled(&fixture, 0).await;
        assert!(recaptured.candidate.is_none(), "{recaptured:?}");
        assert!(
            recaptured
                .units
                .values()
                .all(|unit| *unit == UnitApplication::Applied),
            "{recaptured:?}"
        );
        assert!(!Arc::ptr_eq(
            before.capability(),
            runtime.runtime_resources().capability()
        ));
        assert_eq!(runtime.model_view().configured, model("local/c"));
        let bodies = attempt("after adoption").await;
        assert!(bodies.iter().any(|body| body["model"] == json!("c")));
        let child = issue459_child(&bodies);
        if edit == Issue459AgentEdit::Definition {
            assert_eq!(child["model"], json!("b"));
            assert!(child.to_string().contains("issue459 reviewer v2"));
        } else {
            assert_eq!(
                (&child["model"], &child["temperature"], &child["seed"]),
                (&json!("c"), &json!(0.25), &json!(7))
            );
        }
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
/// publication changes only future selection: new work is refused until
/// `local/c` is selected, and the next child then inherits `local/c`.
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
        let stale = application
            .candidate
            .clone()
            .expect("the Agent change waits");
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

        // Future selection only: new work waits for an explicit correction.
        assert!(matches!(
            runtime.submit_inbound(issue459_text("request-B refused")),
            Err(InboundAdmissionError::ModelUnavailable { .. })
        ));
        fixture
            .manager
            .set_model(&id, model("local/c"))
            .await
            .unwrap();
        assert!(matches!(
            fixture
                .manager
                .adopt_configuration(&id, &stale.identity, stale.expected_binding),
            Err(AdoptionError::Conflict)
        ));
        settled(&fixture, 0).await;
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
        // Still the adopted closure: the old definition, no `subagent`.
        assert!(!child.to_string().contains("issue459 reviewer v2"));
        assert!(
            bodies
                .iter()
                .all(|body| !issue459_tool_names(body).contains(&"subagent".to_owned()))
        );
        fixture.close().await;
    }))
    .await;
}

/// Issue #459: an adopted Agent's changed definition adopts with its unit.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue459_adopted_agent_definition_change_waits_for_capability_adoption() {
    Box::pin(bounded(issue459_explicit_agent_edit(
        Issue459AgentEdit::Definition,
    )))
    .await;
}

/// Issue #459: an adopted Agent's changed explicit model selection adopts
/// with its unit, never with the catalog.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue459_adopted_agent_selection_change_waits_for_capability_adoption() {
    Box::pin(bounded(issue459_explicit_agent_edit(
        Issue459AgentEdit::Selection,
    )))
    .await;
}

/// Issue #459: a catalog that changes what an adopted Agent's own selection
/// resolves to is an invocation change and is never published alone.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue459_catalog_changing_an_adopted_agent_invocation_needs_adoption() {
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

fn issue462_adopt(fixture: &Fixture, application: ConfigurationApplication) {
    if let Some(candidate) = application.candidate {
        fixture
            .manager
            .adopt_configuration(
                &fixture.sessions[0].id,
                &candidate.identity,
                candidate.expected_binding,
            )
            .unwrap();
    }
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
        issue462_adopt(&fixture, replacement);
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
        assert!(invalid.candidate.is_none(), "{invalid:?}");
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
        issue462_adopt(&fixture, issue462_admit(&fixture, &[]).await);
        let from = fixture.provider.request_bodies().len();
        attempt("request-A zero").await;
        let offered = issue462_offered_controls(&fixture, from);
        assert!(!offered.is_empty());
        assert!(
            offered.iter().all(|(names, _)| names.is_empty()),
            "{offered:?}"
        );

        // 0 -> 5 again from the same discovered set.
        issue462_adopt(&fixture, issue462_admit(&fixture, &["reviewer"]).await);
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
