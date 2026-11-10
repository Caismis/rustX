//! Mixed catalog publications that remove a selected model or profile:
//! accepted inbound waits for explicit recovery, the rest of the generation
//! commits automatically, and cold Sessions receive the publication on
//! residency.
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
        applied(&fixture, 0).await;
        fixture
            .manager
            .set_model(&id, issue459_pinned(Some("fast")))
            .await
            .unwrap();
        applied(&fixture, 0).await;
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
        // Queued input owns no committed resource: the publication commits
        // while it waits.
        applied(&fixture, 0).await;
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
/// default and changes the root instructions. The idle Session commits the
/// whole generation automatically: it discovers `local/c`, reports `local/a`
/// unavailable without substituting anything, and refuses new work until
/// `session/setModel(local/c)`; the next Attempt then sends `local/c` with
/// the committed instructions.
#[allow(clippy::too_many_lines)] // one ordered acceptance sequence
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue459_removed_model_with_instructions_change_commits_and_recovers_explicitly() {
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
        issue459_apply_instructions(&fixture, "issue459 adopted instructions").await;
        fixture
            .manager
            .set_model(&id, model("local/a"))
            .await
            .unwrap();
        applied(&fixture, 0).await;
        let retained = || {
            fixture
                .manager
                .sessions
                .configuration_bindings
                .lock()
                .unwrap()[&id]
                .clone()
        };
        let old = runtime.runtime_resources().configuration().unwrap().clone();
        let selected = runtime.model_view();
        issue459_edit(&fixture, |document| {
            issue459_replace_a_with_c(document);
            set_file_guidance(&fixture, "issue459 new instructions");
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
        let committed = runtime.runtime_resources().configuration().unwrap().clone();
        assert_eq!(
            guidance(runtime.runtime_resources().project_context_files()),
            "issue459 new instructions"
        );
        assert_ne!(
            committed.authored.component_revisions[&ApplyUnit::Instructions],
            old.authored.component_revisions[&ApplyUnit::Instructions]
        );
        assert_eq!(
            guidance(&retained().project_context_files),
            "issue459 new instructions"
        );
        assert!(matches!(
            runtime.submit_inbound(issue459_text("refused")),
            Err(InboundAdmissionError::ModelUnavailable { .. })
        ));
        assert!(fixture.provider.request_bodies().is_empty());

        // Explicit recovery keeps the committed instructions.
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
        applied(&fixture, 0).await;
        assert_eq!(runtime.model_view(), view);
        assert_eq!(retained().session_model(), &model("local/c"));
        let mut settlement = SettlementReceiptWait::capture(&runtime);
        runtime.submit_inbound(issue459_text("on c")).unwrap();
        fixture.gates[0].wait_entered().await;
        fixture.gates[0].release();
        settlement.wait().await;
        assert_eq!(fixture.provider.request_bodies().len(), 1);
        let body = issue459_body(&fixture, 0);
        assert_eq!(
            (&body["model"], &body["temperature"], &body["seed"]),
            (&json!("c"), &json!(0.25), &json!(7))
        );
        assert!(fixture.provider.request_bodies()[0].contains("issue459 new instructions"));
        drop(client);
        fixture.close().await;
    }))
    .await;
}

/// Issue #459: the mixed contract on a cold Session. The generation that
/// removes the selected `local/a` and changes the instructions arrives while
/// the Session is not resident: nothing is composed for it and its retained
/// binding is untouched. Natural residency publishes and commits it exactly
/// as for a resident Session, and explicit recovery keeps the commit.
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
        issue459_apply_instructions(&fixture, "issue459 adopted instructions").await;
        fixture
            .manager
            .set_model(&id, model("local/a"))
            .await
            .unwrap();
        applied(&fixture, 0).await;
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
            set_file_guidance(&fixture, "issue459 new instructions");
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
            guidance(&cold.project_context_files),
            "issue459 adopted instructions"
        );

        // Residency publishes the catalog and commits the generation.
        fixture.manager.load(&id, None).await.unwrap();
        let runtime = fixture.manager.configuration_runtime(&id).unwrap();
        let application = applied(&fixture, 0).await;
        assert!(
            application
                .units
                .values()
                .all(|unit| *unit == UnitApplication::Applied),
            "{application:?}"
        );
        assert_eq!(issue459_model_refs(&runtime), ["local/b", "local/c"]);
        let view = runtime.model_view();
        assert_eq!(view.configured, model("local/a"));
        assert!(view.unavailable.as_deref().unwrap().contains("local/a"));
        assert_eq!(
            guidance(runtime.runtime_resources().project_context_files()).to_owned(),
            "issue459 new instructions"
        );
        assert!(matches!(
            runtime.submit_inbound(issue459_text("refused")),
            Err(InboundAdmissionError::ModelUnavailable { .. })
        ));

        // Explicit recovery keeps the committed generation.
        let view = fixture
            .manager
            .set_model(&id, model("local/c"))
            .await
            .unwrap()
            .0;
        assert!(view.unavailable.is_none());
        applied(&fixture, 0).await;
        assert_eq!(runtime.model_view(), view);
        assert_eq!(retained().session_model(), &model("local/c"));
        assert_eq!(
            guidance(runtime.runtime_resources().project_context_files()),
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
/// generation removes the pinned `fast`, adds `deep`, drops the admitted
/// `job_status` tool and admits `reviewer`. The idle Session commits the
/// complete capability closure automatically and reports `fast` unavailable;
/// selecting `deep` then runs on the committed tools, and the replaced
/// closure is retired once its leases settle.
#[allow(clippy::too_many_lines)] // one ordered acceptance sequence
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue459_removed_profile_with_capability_change_commits_and_recovers_explicitly() {
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
        applied(&fixture, 0).await;
        fixture
            .manager
            .set_model(&id, issue459_pinned(Some("fast")))
            .await
            .unwrap();
        applied(&fixture, 0).await;
        let before = runtime.runtime_resources();
        let capability = before.capability().clone();
        let selected = runtime.model_view();
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
        let application = applied(&fixture, 0).await;
        assert!(
            application
                .units
                .values()
                .all(|unit| *unit == UnitApplication::Applied),
            "{application:?}"
        );

        // Discoverable on both surfaces, the removed Profile explicit.
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
        // The complete capability closure was replaced.
        let committed = runtime.runtime_resources();
        assert!(!Arc::ptr_eq(
            capability.tool_registry(),
            committed.capability().tool_registry()
        ));
        assert!(
            committed
                .capability()
                .tool_registry()
                .definitions()
                .iter()
                .all(|tool| tool.name != "job_status")
        );
        assert_eq!(
            committed
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
        let change = configuration_changes(&runtime).pop().unwrap();
        assert!(
            change
                .tools
                .removed
                .names
                .contains(&"job_status".to_owned())
        );

        // `deep` is selected against the committed capabilities.
        let view = fixture
            .manager
            .set_model(&id, issue459_pinned(Some("deep")))
            .await
            .unwrap()
            .0;
        assert!(view.unavailable.is_none());
        applied(&fixture, 0).await;
        assert_eq!(runtime.model_view(), view);
        let binding = fixture
            .manager
            .sessions
            .configuration_bindings
            .lock()
            .unwrap()[&id]
            .clone();
        assert_eq!(binding.session_model(), &issue459_pinned(Some("deep")));
        // The older closure is retired: no runtime owner holds its tools.
        let registry = capability.tool_registry().clone();
        drop((before, capability, committed));
        runtime.settle_configuration_resources().await;
        assert_eq!(Arc::strong_count(&registry), 1);
        let mut settlement = SettlementReceiptWait::capture(&runtime);
        runtime.submit_inbound(issue459_text("on deep")).unwrap();
        fixture.gates[0].wait_entered().await;
        fixture.gates[0].release();
        settlement.wait().await;
        let body = issue459_body(&fixture, 0);
        assert_eq!(
            (&body["temperature"], &body["seed"]),
            (&json!(0.1), &json!(3))
        );
        assert!(!issue459_tool_names(&body).contains(&"job_status".to_owned()));
        drop(client);
        fixture.close().await;
    }))
    .await;
}
