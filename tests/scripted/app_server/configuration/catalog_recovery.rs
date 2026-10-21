//! Mixed catalog publications that remove a selected model or profile:
//! accepted inbound waits for explicit recovery, recovery precedes adoption,
//! and cold Sessions receive the publication on residency.
#![allow(clippy::large_futures)] // bounded fixture futures; no recursive or unbounded stack growth
use super::*;

/// Issue #459: inbound accepted before a publication makes the selection
/// unavailable is held, never run on the removed Profile and never dropped.
/// The admission pass for it is parked before the coordinator lock while the
/// publication commits, so it observes the unavailable selection. Held work
/// does not make the Session busy for the explicit correction — otherwise the
/// selection could never be fixed — and committing a valid selection admits
/// it with exactly that selection's parameters.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue459_inbound_accepted_before_publication_waits_for_explicit_recovery() {
    use serde_json::json;
    Box::pin(bounded(async {
        let fixture = Fixture::with_session_count(None, 1).await;
        let id = fixture.sessions[0].id.clone();
        fixture.manager.load(&id, None).await.unwrap();
        let runtime = fixture.manager.configuration_runtime(&id).unwrap();
        let host = fixture.manager.configuration_host(&id).unwrap();
        let authored = issue459_authored_a(&fixture).await;
        let fast = ("fast", json!({"temperature": 1.0}));
        let deep = ("deep", json!({"temperature": 0.1, "seed": 9}));
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
        fixture
            .manager
            .set_model(&id, issue459_pinned(Some("fast")))
            .await
            .unwrap();
        settled(&fixture, 0).await;
        fixture.gates[0].release();

        // Accepted while `fast` is published; its admission pass parks.
        let gate = Arc::new(crate::runtime::conversation_runtime::Gate::default());
        let release = gate.arm_scoped();
        runtime.install_configuration_admission_gate(gate.clone());
        runtime.submit_inbound(issue459_text("queued")).unwrap();
        tokio::task::spawn_blocking(move || gate.wait_entered())
            .await
            .unwrap();
        write(
            &fixture,
            0,
            issue459_profiles(&authored, "deep", std::slice::from_ref(&deep)),
        )
        .await;
        settled(&fixture, 0).await;
        assert!(runtime.model_view().unavailable.is_some());
        drop(release);

        // The held item is not run on the removed Profile, and it does not
        // block the correction; committing the correction admits it.
        let mut settlement = SettlementReceiptWait::capture(&runtime);
        fixture
            .manager
            .set_model(&id, issue459_pinned(Some("deep")))
            .await
            .unwrap();
        settlement.wait().await;
        let bodies = fixture.provider.request_bodies();
        assert_eq!(bodies.len(), 1, "exactly one request, on the replacement");
        let held: serde_json::Value = serde_json::from_str(&bodies[0]).unwrap();
        assert_eq!(
            (&held["temperature"], &held["seed"]),
            (&json!(0.1), &json!(9))
        );
        assert!(bodies[0].contains("queued"));
        assert!(
            host.checkpoint()
                .map(|checkpoint| (checkpoint.snapshot, checkpoint.cursor))
                .unwrap()
                .0
                .inbound
                .pending
                .is_empty()
        );
        fixture.close().await;
    }))
    .await;
}

/// Issue #459, the mixed-generation acceptance contract. One valid source
/// generation removes the selected `local/a`, adds `local/c` as the new
/// default and changes the root instructions. The Provider unit publishes on
/// its own: the Session discovers `local/c` and reports `local/a`
/// unavailable while its adopted instructions, capabilities and source
/// baseline stay exactly as they were and the Instructions change waits,
/// prepared, for adoption. `session/setModel(local/c)` succeeds without that
/// adoption; the next Attempt sends `local/c` with the still-adopted
/// instructions; the older candidate can no longer adopt over the selection;
/// the recaptured Instructions change then adopts and keeps `local/c`.
#[allow(clippy::too_many_lines)] // one ordered acceptance sequence
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue459_removed_model_with_instructions_change_recovers_before_adoption() {
    use crate::model::catalog::ModelRef;
    use crate::model::session::SessionModelConfig;
    use crate::runtime::conversation_runtime::InboundAdmissionError;
    use serde_json::json;
    Box::pin(bounded(async {
        let fixture = Fixture::new().await;
        let id = fixture.sessions[0].id.clone();
        fixture.manager.load(&id, None).await.unwrap();
        let runtime = fixture.manager.configuration_runtime(&id).unwrap();
        let host = fixture.manager.configuration_host(&id).unwrap();
        let (client, _) = host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
            .unwrap();
        let model = |id: &str| SessionModelConfig::of(ModelRef::parse(id).unwrap());
        issue459_adopt_instructions(&fixture, "issue459 adopted instructions").await;
        fixture
            .manager
            .set_model(&id, model("local/a"))
            .await
            .unwrap();
        settled(&fixture, 0).await;
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
        let selected = runtime.model_view();
        let binding =
            crate::local_runtime::configuration::settings::EffectiveConfiguration::adopted(
                &runtime,
            )
            .unwrap()
            .adopted_binding;
        assert_eq!(retained().binding_revision, binding);
        let mut eligibility = EligibilityStream::open(&fixture, &id);

        issue459_edit(&fixture, |document| {
            issue459_replace_a_with_c(document);
            issue459_set_instructions(document, "issue459 new instructions");
        })
        .await;
        let application = settled(&fixture, 0).await;
        let published_revision = application.desired.input_revision.clone().unwrap();
        assert_eq!(
            application.units[&ApplyUnit::Provider],
            UnitApplication::Applied,
            "{application:?}"
        );
        assert_eq!(
            application.units[&ApplyUnit::Capabilities],
            UnitApplication::Applied,
            "{application:?}"
        );
        assert!(
            matches!(
                application.units[&ApplyUnit::Instructions],
                UnitApplication::Ready { .. }
            ),
            "{application:?}"
        );
        let stale = application.candidate.clone().expect("instructions wait");
        // Prepared against the published binding, never the older one.
        assert_eq!(stale.expected_binding, binding + 1);

        // Discovery agrees on both native surfaces: `local/c`, never `local/a`.
        let catalog =
            super::super::protocol::read_native_catalog_result(client.model_catalog().unwrap());
        assert_eq!(catalog, runtime.model_catalog());
        assert_eq!(issue459_model_refs(&runtime), ["local/b", "local/c"]);
        // The Session keeps `local/a`, reported unavailable, nothing substituted.
        let view = runtime.model_view();
        assert_eq!(view.configured, model("local/a"));
        assert!(view.unavailable.as_deref().unwrap().contains("local/a"));
        assert_eq!(view.effective, selected.effective);
        // Nothing but the Provider unit moved.
        let published = runtime.runtime_resources();
        assert!(Arc::ptr_eq(before.capability(), published.capability()));
        let new = published.configuration().unwrap().clone();
        assert_eq!(
            new.config.agent.instructions,
            "issue459 adopted instructions"
        );
        assert_eq!(
            crate::local_runtime::configuration::settings::EffectiveConfiguration::adopted(
                &runtime
            )
            .unwrap()
            .root_agent
            .instructions,
            "issue459 adopted instructions"
        );
        assert_eq!(new.authored.source_revisions, old.authored.source_revisions);
        assert_eq!(
            new.authored.component_revisions[&ApplyUnit::Provider],
            published_revision
        );
        assert_eq!(
            new.authored.component_revisions[&ApplyUnit::Instructions],
            old.authored.component_revisions[&ApplyUnit::Instructions]
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
        assert_eq!(
            binding_now.config().agent.instructions,
            "issue459 adopted instructions"
        );
        assert_eq!(binding_now.source_revisions, old.authored.source_revisions);
        assert!(matches!(
            runtime.submit_inbound(issue459_text("refused")),
            Err(InboundAdmissionError::ModelUnavailable { .. })
        ));
        assert!(fixture.provider.request_bodies().is_empty());

        // Explicit recovery without adopting the Instructions candidate.
        let view = fixture
            .manager
            .set_model(&id, model("local/c"))
            .await
            .unwrap()
            .0;
        assert!(view.unavailable.is_none());
        let effective = view.effective.as_ref().unwrap();
        assert_eq!(effective.model.to_string(), "local/c");
        assert_eq!(effective.profile.as_ref().unwrap().as_str(), "steady");
        // The older candidate can no longer adopt over that selection.
        assert!(matches!(
            fixture
                .manager
                .adopt_configuration(&id, &stale.identity, stale.expected_binding),
            Err(AdoptionError::Conflict)
        ));
        assert_eq!(runtime.model_view(), view);
        // The Instructions change is not lost: it is recaptured over `local/c`.
        let pending = settled(&fixture, 0).await;
        let candidate = pending.candidate.clone().expect("recaptured instructions");
        assert_ne!(candidate.identity, stale.identity);
        assert_eq!(
            pending.units[&ApplyUnit::Provider],
            UnitApplication::Applied
        );

        fixture.gates[0].release();
        let mut settlement = SettlementReceiptWait::capture(&runtime);
        runtime.submit_inbound(issue459_text("on c")).unwrap();
        settlement.wait().await;
        let body = issue459_body(&fixture, 0);
        assert_eq!(
            (&body["model"], &body["temperature"], &body["seed"]),
            (&json!("c"), &json!(0.25), &json!(7))
        );
        let sent = fixture.provider.request_bodies()[0].clone();
        assert!(sent.contains("issue459 adopted instructions"));
        assert!(!sent.contains("issue459 new instructions"));

        // The Instructions change adopts and keeps the corrected selection.
        eligibility.until(AdoptionEligibility::Busy).await;
        eligibility.until(AdoptionEligibility::Eligible).await;
        fixture
            .manager
            .adopt_configuration(&id, &candidate.identity, candidate.expected_binding)
            .unwrap();
        assert_eq!(runtime.model_view(), view);
        assert_eq!(retained().session_model(), &model("local/c"));
        let adopted = runtime.runtime_resources().configuration().unwrap().clone();
        assert_eq!(
            adopted.config.agent.instructions,
            "issue459 new instructions"
        );
        assert_ne!(
            adopted.authored.source_revisions,
            old.authored.source_revisions
        );
        let mut settlement = SettlementReceiptWait::capture(&runtime);
        runtime
            .submit_inbound(issue459_text("after adoption"))
            .unwrap();
        settlement.wait().await;
        let body = issue459_body(&fixture, 1);
        assert_eq!(
            (&body["model"], &body["temperature"]),
            (&json!("c"), &json!(0.25))
        );
        assert!(fixture.provider.request_bodies()[1].contains("issue459 new instructions"));
        drop(client);
        fixture.close().await;
    }))
    .await;
}

/// Issue #459: the mixed contract on a cold Session. The generation that
/// removes the selected `local/a` and changes the instructions arrives while
/// the Session is not resident: nothing is composed for it and its retained
/// binding is untouched. Natural residency publishes the catalog on its own,
/// the Instructions change waits for adoption, and recovery and adoption then
/// proceed exactly as for a resident Session.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue459_cold_session_receives_mixed_publication_on_residency() {
    use crate::model::catalog::ModelRef;
    use crate::model::session::SessionModelConfig;
    use crate::runtime::conversation_runtime::InboundAdmissionError;
    Box::pin(bounded(async {
        let fixture = Fixture::with_session_count(None, 1).await;
        let id = fixture.sessions[0].id.clone();
        let conversation = fixture.sessions[0].active_conversation_id.clone();
        fixture.manager.load(&id, None).await.unwrap();
        let model = |id: &str| SessionModelConfig::of(ModelRef::parse(id).unwrap());
        issue459_adopt_instructions(&fixture, "issue459 adopted instructions").await;
        fixture
            .manager
            .set_model(&id, model("local/a"))
            .await
            .unwrap();
        settled(&fixture, 0).await;
        let retained = || {
            fixture
                .manager
                .sessions
                .configuration_bindings
                .lock()
                .unwrap()[&id]
                .clone()
        };
        let before = retained();
        fixture.manager.unload(&conversation).await.unwrap();
        assert!(fixture.manager.configuration_runtime(&id).is_none());

        let mut changed = fixture.manager.configuration_changes();
        issue459_edit(&fixture, |document| {
            issue459_replace_a_with_c(document);
            issue459_set_instructions(document, "issue459 new instructions");
        })
        .await;
        while !fixture.manager.applications.is_deferred(id.as_str()) {
            changed.changed().await.unwrap();
        }
        // Cold: nothing composed, the retained binding is exactly as it was.
        assert!(fixture.manager.configuration_runtime(&id).is_none());
        let cold = retained();
        assert_eq!(cold.binding_revision, before.binding_revision);
        assert_eq!(cold.source_revisions, before.source_revisions);
        assert_eq!(
            cold.config().agent.instructions,
            "issue459 adopted instructions"
        );
        assert!(
            fixture
                .manager
                .configuration_application(&id)
                .unwrap()
                .candidate
                .is_none()
        );

        // Residency publishes the catalog apart from the Instructions unit.
        fixture.manager.load(&id, None).await.unwrap();
        let runtime = fixture.manager.configuration_runtime(&id).unwrap();
        let application = session_settled(&fixture, &id).await;
        assert_eq!(
            application.units[&ApplyUnit::Provider],
            UnitApplication::Applied,
            "{application:?}"
        );
        assert!(
            matches!(
                application.units[&ApplyUnit::Instructions],
                UnitApplication::Ready { .. }
            ),
            "{application:?}"
        );
        let stale = application.candidate.expect("instructions wait");
        assert_eq!(issue459_model_refs(&runtime), ["local/b", "local/c"]);
        let view = runtime.model_view();
        assert_eq!(view.configured, model("local/a"));
        assert!(view.unavailable.as_deref().unwrap().contains("local/a"));
        let published = retained();
        assert_eq!(published.binding_revision, before.binding_revision + 1);
        assert_eq!(published.source_revisions, before.source_revisions);
        assert_eq!(
            crate::local_runtime::configuration::settings::EffectiveConfiguration::adopted(
                &runtime
            )
            .unwrap()
            .root_agent
            .instructions,
            "issue459 adopted instructions"
        );
        assert!(matches!(
            runtime.submit_inbound(issue459_text("refused")),
            Err(InboundAdmissionError::ModelUnavailable { .. })
        ));

        // Explicit recovery, then the recaptured Instructions change adopts.
        let view = fixture
            .manager
            .set_model(&id, model("local/c"))
            .await
            .unwrap()
            .0;
        assert!(view.unavailable.is_none());
        assert!(matches!(
            fixture
                .manager
                .adopt_configuration(&id, &stale.identity, stale.expected_binding),
            Err(AdoptionError::Conflict)
        ));
        let candidate = session_settled(&fixture, &id)
            .await
            .candidate
            .expect("recaptured instructions");
        fixture
            .manager
            .adopt_configuration(&id, &candidate.identity, candidate.expected_binding)
            .unwrap();
        assert_eq!(runtime.model_view(), view);
        assert_eq!(retained().session_model(), &model("local/c"));
        assert_eq!(
            runtime
                .runtime_resources()
                .configuration()
                .unwrap()
                .config
                .agent
                .instructions,
            "issue459 new instructions"
        );
        assert!(fixture.provider.request_bodies().is_empty());
        fixture.close().await;
    }))
    .await;
}

fn issue459_replace_profile(document: &mut toml::Value, removed: &str, added: (&str, &str)) {
    let profiles = document["models"]["local/a"]["profiles"]
        .as_table_mut()
        .unwrap();
    profiles.remove(removed).unwrap();
    let mut profile = toml::Table::new();
    profile.insert("request_params".into(), added.1.into());
    profiles.insert(added.0.into(), profile.into());
}

/// Issue #459: the removed-Profile form of the mixed contract. One source
/// generation removes the pinned `fast`, adds `deep` and drops the admitted
/// `job_status` tool. The catalog is discoverable before the Capability
/// change is adopted and the Session reports `fast` unavailable; the
/// capability closure is untouched, so selecting `deep` runs on the adopted
/// tools — a Model selection never authorizes an unadopted Capability. The
/// recaptured Capability candidate then adopts, retiring the older closure
/// and keeping `deep`.
#[allow(clippy::too_many_lines)] // one ordered acceptance sequence
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue459_removed_profile_with_capability_change_recovers_before_adoption() {
    use serde_json::json;
    Box::pin(bounded(async {
        let fixture = Fixture::new().await;
        let id = fixture.sessions[0].id.clone();
        issue459_reviewer_sources(&fixture);
        fixture.manager.load(&id, None).await.unwrap();
        let runtime = fixture.manager.configuration_runtime(&id).unwrap();
        let host = fixture.manager.configuration_host(&id).unwrap();
        let (client, _) = host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
            .unwrap();
        let authored = issue459_authored_a(&fixture).await;
        write(
            &fixture,
            0,
            issue459_profiles(
                &authored,
                "balanced",
                &[
                    ("balanced", json!({"temperature": 0.5})),
                    ("fast", json!({"temperature": 1.0})),
                ],
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
            .set_model(&id, issue459_pinned(Some("fast")))
            .await
            .unwrap();
        settled(&fixture, 0).await;
        let before = runtime.runtime_resources();
        let capability = before.capability().clone();
        let selected = runtime.model_view();
        let mut eligibility = EligibilityStream::open(&fixture, &id);

        issue459_edit(&fixture, |document| {
            issue459_replace_profile(
                document,
                "fast",
                ("deep", "{\"temperature\":0.1,\"seed\":3}"),
            );
            issue459_remove_builtin_tool(document, "job_status");
            issue459_admit_reviewer(document);
        })
        .await;
        let application = settled(&fixture, 0).await;
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
        let stale = application.candidate.clone().expect("capabilities wait");

        // Discoverable before the Capability adoption, on both surfaces.
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
        let view = runtime.model_view();
        assert_eq!(view.configured, issue459_pinned(Some("fast")));
        assert!(view.unavailable.as_deref().unwrap().contains("fast"));
        assert_eq!(view.effective, selected.effective);
        // The capability closure is not replaced.
        assert!(Arc::ptr_eq(
            &capability,
            runtime.runtime_resources().capability()
        ));
        assert!(
            capability
                .tool_registry()
                .definitions()
                .iter()
                .any(|tool| tool.name == "job_status")
        );
        // `reviewer` is desired, not adopted.
        assert!(
            runtime
                .runtime_resources()
                .configuration()
                .unwrap()
                .config
                .agent
                .agents
                .is_empty()
        );

        // `deep` is selected against the adopted capabilities.
        let view = fixture
            .manager
            .set_model(&id, issue459_pinned(Some("deep")))
            .await
            .unwrap()
            .0;
        assert!(view.unavailable.is_none());
        assert!(matches!(
            fixture
                .manager
                .adopt_configuration(&id, &stale.identity, stale.expected_binding),
            Err(AdoptionError::Conflict)
        ));
        // The selection re-wraps nothing it does not own: the same tools.
        assert!(Arc::ptr_eq(
            capability.tool_registry(),
            runtime.runtime_resources().capability().tool_registry()
        ));
        let pending = settled(&fixture, 0).await;
        let candidate = pending.candidate.clone().expect("recaptured capabilities");
        assert_ne!(candidate.identity, stale.identity);
        fixture.gates[0].release();
        let mut settlement = SettlementReceiptWait::capture(&runtime);
        runtime.submit_inbound(issue459_text("on deep")).unwrap();
        settlement.wait().await;
        let body = issue459_body(&fixture, 0);
        assert_eq!(
            (&body["temperature"], &body["seed"]),
            (&json!(0.1), &json!(3))
        );
        assert!(issue459_tool_names(&body).contains(&"job_status".to_owned()));

        // The Capability change adopts through the ordinary flow.
        eligibility.until(AdoptionEligibility::Busy).await;
        eligibility.until(AdoptionEligibility::Eligible).await;
        fixture
            .manager
            .adopt_configuration(&id, &candidate.identity, candidate.expected_binding)
            .unwrap();
        let adopted = runtime.runtime_resources();
        assert!(!Arc::ptr_eq(
            capability.tool_registry(),
            adopted.capability().tool_registry()
        ));
        assert!(
            adopted
                .capability()
                .tool_registry()
                .definitions()
                .iter()
                .all(|tool| tool.name != "job_status")
        );
        assert_eq!(runtime.model_view(), view);
        assert_eq!(
            adopted
                .configuration()
                .unwrap()
                .config
                .agent
                .agents
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
            ["reviewer"]
        );
        let binding = fixture
            .manager
            .sessions
            .configuration_bindings
            .lock()
            .unwrap()[&id]
            .clone();
        assert_eq!(binding.session_model(), &issue459_pinned(Some("deep")));
        assert_eq!(
            binding.component_revisions,
            adopted
                .configuration()
                .unwrap()
                .authored
                .component_revisions
        );
        // The older closure is retired: no runtime owner holds its tools.
        let registry = capability.tool_registry().clone();
        drop((before, capability));
        runtime.settle_configuration_resources().await;
        assert_eq!(Arc::strong_count(&registry), 1);
        let mut settlement = SettlementReceiptWait::capture(&runtime);
        runtime
            .submit_inbound(issue459_text("after adoption"))
            .unwrap();
        settlement.wait().await;
        let body = issue459_body(&fixture, 1);
        assert_eq!(body["temperature"], json!(0.1));
        assert!(!issue459_tool_names(&body).contains(&"job_status".to_owned()));
        drop(client);
        fixture.close().await;
    }))
    .await;
}
