//! Mixed catalog generations: admitted attempts stay frozen, selections
//! before and after publication, failed and overtaken generations never
//! restore authority, and clients agree on the summary policy.
#![allow(clippy::large_futures)] // bounded fixture futures; no recursive or unbounded stack growth
use super::*;

/// Issue #459: an Attempt admitted before a mixed publication stays frozen.
/// It is parked at its first provider response while one generation removes
/// its `local/a` and changes the instructions; the Session's future selection
/// authority advances (the catalog, `local/a` unavailable) while the Attempt
/// snapshot does not, and both its Tool proposal and the continuation are
/// sent with `local/a` and the instructions it froze. Its settlement commits
/// the staged instructions; `local/c` is then selected explicitly and the
/// next Attempt freezes `local/c` with the committed instructions.
#[allow(clippy::too_many_lines)] // one ordered acceptance sequence
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue459_attempt_admitted_before_mixed_publication_stays_frozen() {
    use crate::model::catalog::ModelRef;
    use crate::model::session::SessionModelConfig;
    use serde_json::json;
    Box::pin(bounded(async {
        let fixture = Fixture::with_session_count(Some("read"), 1).await;
        let id = fixture.sessions[0].id.clone();
        fixture.manager.load(&id, None).await.unwrap();
        let runtime = fixture.manager.configuration_runtime(&id).unwrap();
        let host = fixture.manager.configuration_host(&id).unwrap();
        let model = |id: &str| SessionModelConfig::of(ModelRef::parse(id).unwrap());
        issue459_apply_instructions(&fixture, "issue459 adopted instructions").await;
        fixture
            .manager
            .set_model(&id, model("local/a"))
            .await
            .unwrap();
        settled(&fixture, 0).await;
        let invocation = runtime.model_snapshot();
        let tools = runtime
            .runtime_resources()
            .capability()
            .tool_registry()
            .clone();
        let mut settlement = SettlementReceiptWait::capture(&runtime);
        runtime.submit_inbound(issue459_text("request-A")).unwrap();
        fixture.gates[0].wait_entered().await;
        let admitted = host
            .checkpoint()
            .map(|checkpoint| (checkpoint.snapshot, checkpoint.cursor))
            .unwrap()
            .0
            .attempt
            .unwrap();

        issue459_edit(&fixture, |document| {
            issue459_replace_a_with_c(document);
            set_file_guidance(&fixture, "issue459 new instructions");
        })
        .await;
        let application = settled(&fixture, 0).await;
        assert_eq!(
            application.units[&ApplyUnit::Provider],
            UnitApplication::Applied,
            "{application:?}"
        );
        assert_eq!(
            application.units[&ApplyUnit::Instructions],
            UnitApplication::Ready,
            "{application:?}"
        );
        // Future selection authority advanced; the admitted Attempt did not.
        assert_eq!(issue459_model_refs(&runtime), ["local/b", "local/c"]);
        assert!(runtime.model_view().unavailable.is_some());
        let running = host
            .checkpoint()
            .map(|checkpoint| (checkpoint.snapshot, checkpoint.cursor))
            .unwrap()
            .0
            .attempt
            .unwrap();
        assert_eq!(running.model, admitted.model);
        assert_eq!(running.attempt_id, admitted.attempt_id);

        fixture.gates[0].release();
        settlement.wait().await;
        let bodies = fixture.provider.request_bodies();
        assert_eq!(bodies.len(), 2, "Tool proposal and its continuation");
        for sent in &bodies {
            let body: serde_json::Value = serde_json::from_str(sent).unwrap();
            assert_eq!(body["model"], json!("a"));
            assert!(sent.contains("issue459 adopted instructions"));
            assert!(!sent.contains("issue459 new instructions"));
        }
        let continuation: serde_json::Value = serde_json::from_str(&bodies[1]).unwrap();
        assert!(
            continuation["messages"]
                .as_array()
                .unwrap()
                .iter()
                .any(|message| message["role"] == "tool")
        );
        // The frozen invocation is still the one the Session last resolved.
        assert_eq!(
            runtime.model_view().effective,
            Some(invocation.view().primary)
        );

        applied(&fixture, 0).await;
        let view = fixture
            .manager
            .set_model(&id, model("local/c"))
            .await
            .unwrap()
            .0;
        assert!(view.unavailable.is_none());
        applied(&fixture, 0).await;
        let mut settlement = SettlementReceiptWait::capture(&runtime);
        runtime.submit_inbound(issue459_text("on c")).unwrap();
        settlement.wait().await;
        let bodies = fixture.provider.request_bodies();
        assert_eq!(bodies.len(), 3);
        let body: serde_json::Value = serde_json::from_str(&bodies[2]).unwrap();
        assert_eq!(
            (&body["model"], &body["temperature"], &body["seed"]),
            (&json!("c"), &json!(0.25), &json!(7))
        );
        assert!(bodies[2].contains("issue459 new instructions"));
        assert!(!bodies[2].contains("issue459 adopted instructions"));
        assert!(Arc::ptr_eq(
            &tools,
            runtime.runtime_resources().capability().tool_registry()
        ));
        fixture.close().await;
    }))
    .await;
}

/// Model-bearing observations exactly as a Runtime Client attachment
/// receives them: `("model", view)` for a selection commit and
/// `("resources", view)` for a generation commit, in stream order.
struct Issue459Observations {
    _attachment: crate::runtime_client::RuntimeAttachment,
    subscription: crate::runtime_client::EventSubscription,
    conversation_id: crate::runtime::identity::ConversationId,
    resource_revision: crate::runtime::identity::RuntimeResourceRevision,
    model_binding: Option<u64>,
}

impl Issue459Observations {
    fn open(fixture: &Fixture, id: &crate::local_runtime::session::SessionId) -> Self {
        let host = fixture.manager.configuration_host(id).unwrap();
        let (attachment, _) = host
            .attach_read_only(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
            .unwrap();
        let (snapshot, cursor) = host
            .checkpoint()
            .map(|checkpoint| (checkpoint.snapshot, checkpoint.cursor))
            .unwrap();
        let subscription = attachment.subscribe_events(cursor).unwrap();
        Self {
            _attachment: attachment,
            subscription,
            conversation_id: snapshot.conversation_id,
            resource_revision: snapshot.resources.revision,
            model_binding: snapshot
                .model_status
                .and_then(|status| status.binding_revision),
        }
    }

    /// Every model-bearing observation through the first selection commit
    /// that configures `marker`.
    async fn through(
        &mut self,
        marker: &crate::model::session::SessionModelConfig,
    ) -> Vec<(&'static str, crate::model::session::SessionModelView)> {
        use crate::runtime_client::{EventDelivery, RuntimeClientEvent};
        let mut observed = Vec::new();
        loop {
            let EventDelivery::Event(event) = self.subscription.next().await else {
                panic!("observation stream ended")
            };
            for fact in event.event.facts() {
                match fact {
                    RuntimeClientEvent::SessionModelChanged {
                        model,
                        model_status,
                    } => {
                        let model = model.as_deref().expect("finite model fixture stays inline");
                        self.model_binding = model_status.binding_revision;
                        let done = model.configured == *marker;
                        observed.push(("model", model.clone()));
                        if done {
                            return observed;
                        }
                    }
                    RuntimeClientEvent::ResourceGenerationUpdated {
                        model,
                        model_status,
                        resources,
                        ..
                    } => {
                        self.resource_revision = resources.revision;
                        self.model_binding = model_status.binding_revision;
                        observed.push((
                            "resources",
                            model
                                .as_deref()
                                .expect("finite resource model fixture stays inline")
                                .clone(),
                        ));
                    }
                    RuntimeClientEvent::CheckpointAdvanced { checkpoint, .. } => {
                        assert_eq!(checkpoint.snapshot.conversation_id, self.conversation_id);
                        assert_eq!(checkpoint.cursor, event.cursor);
                        let revision = checkpoint.snapshot.resources.revision;
                        let binding = checkpoint
                            .snapshot
                            .model_status
                            .and_then(|status| status.binding_revision);
                        // These are the exact native owner revisions carried by
                        // the complete replacement, not a guessed event label.
                        let kind = if revision != self.resource_revision {
                            Some("resources")
                        } else if binding != self.model_binding {
                            Some("model")
                        } else {
                            None
                        };
                        self.resource_revision = revision;
                        self.model_binding = binding;
                        if let Some(kind) = kind {
                            let model = checkpoint
                                .snapshot
                                .model
                                .as_ref()
                                .expect("finite fixture model in native replacement");
                            let done = kind == "model" && model.configured == *marker;
                            observed.push((kind, model.clone()));
                            eprintln!("model replacement at {:?}: {kind}", checkpoint.cursor);
                            if done {
                                return observed;
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }
}

/// The two orderings of one race: a generation that removes Profile `fast`,
/// changes the instructions and admits the Named Agent `reviewer`, and
/// `session/setModel(fast)` from a Session on `balanced`. The desired Agent
/// never holds the catalog back and commits with its closure; a commit never
/// overwrites the newer explicit selection.
#[allow(clippy::too_many_lines)] // both orderings share one setup and one ledger
async fn issue459_publication_selection_race(selection_first: bool) {
    use serde_json::json;
    let fixture = Fixture::new().await;
    let id = fixture.sessions[0].id.clone();
    issue459_reviewer_sources(&fixture);
    fixture.manager.load(&id, None).await.unwrap();
    let runtime = fixture.manager.configuration_runtime(&id).unwrap();
    let probe = fixture
        .manager
        .probe(&fixture.sessions[0].active_conversation_id);
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
                ("deep", json!({"temperature": 0.1})),
            ],
        ),
    )
    .await;
    applied(&fixture, 0).await;
    fixture
        .manager
        .set_model(&id, issue459_pinned(Some("balanced")))
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
            .binding_revision
    };
    let binding = retained();
    let revision = runtime.runtime_resources().revision();
    let mut observations = Issue459Observations::open(&fixture, &id);
    let profiles = || {
        runtime
            .model_catalog()
            .models
            .iter()
            .find(|model| model.model.to_string() == "local/a")
            .unwrap()
            .profiles
            .iter()
            .map(|profile| profile.id.to_string())
            .collect::<Vec<_>>()
    };
    let reviewers = || {
        runtime
            .runtime_resources()
            .configuration()
            .unwrap()
            .config
            .agent
            .agents
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
    };
    let generation = |document: &mut toml::Value| {
        document["models"]["local/a"]["profiles"]
            .as_table_mut()
            .unwrap()
            .remove("fast")
            .unwrap();
        set_file_guidance(&fixture, "issue459 raced instructions");
        issue459_admit_reviewer(document);
    };

    if selection_first {
        probe.before_catalog_publication.arm();
        issue459_edit(&fixture, generation).await;
        probe.before_catalog_publication.entered().await;
        // Linearizes first, against the catalog still published.
        let selected = fixture
            .manager
            .set_model(&id, issue459_pinned(Some("fast")))
            .await
            .unwrap()
            .0;
        assert!(selected.unavailable.is_none());
        probe.before_catalog_publication.release();
        let application = applied(&fixture, 0).await;
        assert!(
            application
                .units
                .values()
                .all(|unit| *unit == UnitApplication::Applied),
            "{application:?}"
        );
        assert!(
            retained() > binding + 1,
            "the selection, then the generation"
        );
        assert_eq!(profiles(), ["balanced", "deep"]);
        let view = runtime.model_view();
        assert_eq!(view.configured, issue459_pinned(Some("fast")));
        assert!(view.unavailable.as_deref().unwrap().contains("fast"));
        // The published removal now refuses the same selection, changing
        // nothing.
        let before = retained();
        assert!(matches!(
            fixture
                .manager
                .set_model(&id, issue459_pinned(Some("fast")))
                .await,
            Err(ModelSelectionError::Model(
                crate::runtime::conversation_runtime::ModelUpdateError::InvalidConfiguration(..)
            ))
        ));
        assert_eq!(retained(), before);
        let marker = issue459_pinned(Some("deep"));
        fixture
            .manager
            .set_model(&id, marker.clone())
            .await
            .unwrap();
        let observed = observations.through(&marker).await;
        assert_eq!(observed[0].0, "model", "{observed:?}");
        assert!(observed[0].1.unavailable.is_none());
        assert!(
            observed.iter().any(|(kind, view)| *kind == "resources"
                && view.configured == issue459_pinned(Some("fast"))
                && view.unavailable.is_some()),
            "{observed:?}"
        );
        applied(&fixture, 0).await;
        assert_eq!(reviewers(), ["reviewer"]);
        assert_eq!(runtime.model_view().configured, marker);
    } else {
        issue459_edit(&fixture, generation).await;
        let application = applied(&fixture, 0).await;
        assert!(
            application
                .units
                .values()
                .all(|unit| *unit == UnitApplication::Applied),
            "{application:?}"
        );
        assert!(runtime.runtime_resources().revision() > revision);
        assert_eq!(profiles(), ["balanced", "deep"]);
        // The unselected removal leaves `balanced` available.
        assert!(runtime.model_view().unavailable.is_none());
        // Linearizes second: the published removal refuses it, changing nothing.
        let view = runtime.model_view();
        let before = retained();
        assert!(matches!(
            fixture
                .manager
                .set_model(&id, issue459_pinned(Some("fast")))
                .await,
            Err(ModelSelectionError::Model(
                crate::runtime::conversation_runtime::ModelUpdateError::InvalidConfiguration(..)
            ))
        ));
        assert_eq!(runtime.model_view(), view);
        assert_eq!(retained(), before);
        assert_eq!(
            fixture.manager.configuration_application(&id).unwrap(),
            application
        );
        let marker = issue459_pinned(Some("deep"));
        fixture
            .manager
            .set_model(&id, marker.clone())
            .await
            .unwrap();
        let observed = observations.through(&marker).await;
        assert!(
            observed[..observed.len() - 1]
                .iter()
                .all(
                    |(_, view)| view.configured == issue459_pinned(Some("balanced"))
                        && view.unavailable.is_none()
                ),
            "{observed:?}"
        );
        applied(&fixture, 0).await;
        assert_eq!(reviewers(), ["reviewer"]);
        assert_eq!(runtime.model_view().configured, marker);
    }
    assert_eq!(
        guidance(runtime.runtime_resources().project_context_files()),
        "issue459 raced instructions"
    );
    assert!(fixture.provider.request_bodies().is_empty());
    fixture.close().await;
}

/// Issue #459: `session/setModel(fast)` linearizes before the publication
/// that removes `fast`. The selection commits against the published catalog;
/// the parked generation is overtaken and recaptured, publishes after it and
/// reports `fast` unavailable; its commit keeps the later explicit selection.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue459_selection_before_mixed_publication_is_reported_unavailable() {
    Box::pin(bounded(issue459_publication_selection_race(true))).await;
}

/// Issue #459: the publication that removes `fast` linearizes before
/// `session/setModel(fast)`. The selection is refused against the published
/// catalog and changes nothing; the generation's commit stands.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue459_selection_after_mixed_publication_is_refused_without_effect() {
    Box::pin(bounded(issue459_publication_selection_race(false))).await;
}

/// Pin `local/a` to `fast` among `balanced`, `fast` and `deep`.
async fn issue459_pin_fast(fixture: &Fixture) {
    use serde_json::json;
    let id = &fixture.sessions[0].id;
    let authored = issue459_authored_a(fixture).await;
    write(
        fixture,
        0,
        issue459_profiles(
            &authored,
            "balanced",
            &[
                ("balanced", json!({"temperature": 0.5})),
                ("fast", json!({"temperature": 1.0})),
                ("deep", json!({"temperature": 0.1})),
            ],
        ),
    )
    .await;
    applied(fixture, 0).await;
    fixture
        .manager
        .set_model(id, issue459_pinned(Some("fast")))
        .await
        .unwrap();
    applied(fixture, 0).await;
}

fn issue459_profile_ids(
    runtime: &crate::runtime::conversation_runtime::ConversationRuntime,
) -> Vec<String> {
    runtime
        .model_catalog()
        .models
        .iter()
        .find(|model| model.model.to_string() == "local/a")
        .unwrap()
        .profiles
        .iter()
        .map(|profile| profile.id.to_string())
        .collect()
}

/// Issue #459: failed mixed generations keep the last good authority, all
/// or nothing, and report exactly which units failed. An invalid catalog (a
/// Model bound to an undeclared Provider) beside a valid Instructions change
/// is an invalid source document: its context capture fails as a whole, so
/// every context unit fails and nothing is published. A valid generation
/// whose retained binding cannot be committed at the fence exposes nothing
/// either: the unchanged capabilities have nothing to apply, while the
/// Provider and the Instructions prepared over it fail. A later good
/// generation publishes.
#[allow(clippy::too_many_lines)] // two failure modes share one last-good ledger
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue459_failed_mixed_generations_keep_last_good_authority() {
    Box::pin(bounded(async {
        let fixture = Fixture::with_session_count(None, 1).await;
        let id = fixture.sessions[0].id.clone();
        fixture.manager.load(&id, None).await.unwrap();
        let runtime = fixture.manager.configuration_runtime(&id).unwrap();
        let probe = fixture
            .manager
            .probe(&fixture.sessions[0].active_conversation_id);
        issue459_apply_instructions(&fixture, "issue459 adopted instructions").await;
        issue459_pin_fast(&fixture).await;
        let ledger = || {
            let binding = fixture
                .manager
                .sessions
                .configuration_bindings
                .lock()
                .unwrap()
                .get(&id)
                .map(|binding| binding.binding_revision);
            (
                runtime.model_view(),
                runtime.model_catalog(),
                runtime.runtime_resources().revision(),
                crate::local_runtime::configuration::settings::EffectiveConfiguration::adopted(
                    &runtime,
                )
                .unwrap()
                .adopted_binding,
                binding,
            )
        };
        let good = ledger();
        let failed = |application: &ConfigurationApplication, capabilities_applied: bool| {
            assert!(
                application
                    .units
                    .values()
                    .all(|unit| *unit != UnitApplication::Ready),
                "{application:?}"
            );
            assert_eq!(
                application.units[&ApplyUnit::Capabilities] == UnitApplication::Applied,
                capabilities_applied,
                "{application:?}"
            );
            let mut failed = vec![ApplyUnit::Provider, ApplyUnit::Instructions];
            if !capabilities_applied {
                failed.push(ApplyUnit::Capabilities);
            }
            for unit in failed {
                assert!(
                    matches!(application.units[&unit], UnitApplication::Failed { .. }),
                    "{application:?}"
                );
            }
            for unit in [ApplyUnit::ExecutionPolicy, ApplyUnit::SharedCapacity] {
                assert_eq!(
                    application.units[&unit],
                    UnitApplication::Applied,
                    "{application:?}"
                );
            }
        };

        // An invalid catalog beside a valid Instructions change.
        issue459_edit(&fixture, |document| {
            let mut model = document["models"]["local/b"].clone();
            model
                .as_table_mut()
                .unwrap()
                .insert("provider".into(), "undeclared".into());
            document["models"]
                .as_table_mut()
                .unwrap()
                .insert("undeclared/x".into(), model);
            set_file_guidance(&fixture, "issue459 unpublished instructions");
        })
        .await;
        let application = settled(&fixture, 0).await;
        failed(&application, false);
        let UnitApplication::Failed { diagnostic } = &application.units[&ApplyUnit::Provider]
        else {
            unreachable!()
        };
        assert!(diagnostic.contains("model catalog"), "{diagnostic}");
        assert_eq!(ledger(), good);
        assert_eq!(
            guidance(runtime.runtime_resources().project_context_files()).to_owned(),
            "issue459 adopted instructions"
        );

        // A valid generation whose retained binding vanishes at the fence.
        probe.before_catalog_publication.arm();
        issue459_edit(&fixture, |document| {
            document["models"]
                .as_table_mut()
                .unwrap()
                .remove("undeclared/x");
            document["models"]["local/a"]["profiles"]
                .as_table_mut()
                .unwrap()
                .remove("fast")
                .unwrap();
        })
        .await;
        probe.before_catalog_publication.entered().await;
        let binding = fixture
            .manager
            .sessions
            .configuration_bindings
            .lock()
            .unwrap()
            .remove(&id)
            .unwrap();
        probe.before_catalog_publication.release();
        let application = settled(&fixture, 0).await;
        failed(&application, true);
        assert_eq!(
            ledger(),
            (good.0.clone(), good.1.clone(), good.2, good.3, None)
        );
        assert!(runtime.model_view().unavailable.is_none());

        // With the binding restored, the same sources publish.
        fixture
            .manager
            .sessions
            .configuration_bindings
            .lock()
            .unwrap()
            .insert(id.clone(), binding);
        fixture
            .manager
            .reconcile_configuration(
                &crate::local_runtime::configuration::settings::SourceTarget::User,
            )
            .await
            .unwrap();
        let application = settled(&fixture, 0).await;
        assert_eq!(
            application.units[&ApplyUnit::Provider],
            UnitApplication::Applied,
            "{application:?}"
        );
        assert_eq!(issue459_profile_ids(&runtime), ["balanced", "deep"]);
        assert!(
            runtime
                .model_view()
                .unavailable
                .as_deref()
                .unwrap()
                .contains("fast")
        );
        assert!(runtime.runtime_resources().revision() > good.2);
        assert!(fixture.provider.request_bodies().is_empty());
        fixture.close().await;
    }))
    .await;
}

/// Issue #459: a valid catalog publishes even when the Capability change of
/// the same generation fails to prepare. The Session reports its removed
/// `fast` unavailable and discovers `deep` while the capability closure
/// stays the committed one; the unchanged instructions are settled. Selecting
/// `deep` recaptures the generation, and the Capability change then
/// prepares and commits automatically.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue459_failed_capability_preparation_keeps_published_catalog_and_retries() {
    Box::pin(bounded(async {
        let fixture = Fixture::with_session_count(None, 1).await;
        let id = fixture.sessions[0].id.clone();
        issue459_reviewer_sources(&fixture);
        fixture.manager.load(&id, None).await.unwrap();
        let runtime = fixture.manager.configuration_runtime(&id).unwrap();
        let probe = fixture
            .manager
            .probe(&fixture.sessions[0].active_conversation_id);
        issue459_pin_fast(&fixture).await;
        let tools = runtime
            .runtime_resources()
            .capability()
            .tool_registry()
            .clone();

        probe.fail_configuration_once.store(true, Ordering::SeqCst);
        issue459_edit(&fixture, |document| {
            document["models"]["local/a"]["profiles"]
                .as_table_mut()
                .unwrap()
                .remove("fast")
                .unwrap();
            issue459_remove_builtin_tool(document, "job_status");
            issue459_admit_reviewer(document);
        })
        .await;
        let application = settled(&fixture, 0).await;
        assert!(
            application
                .units
                .values()
                .all(|unit| *unit != UnitApplication::Ready),
            "{application:?}"
        );
        assert_eq!(
            application.units[&ApplyUnit::Provider],
            UnitApplication::Applied,
            "{application:?}"
        );
        assert_eq!(
            application.units[&ApplyUnit::Instructions],
            UnitApplication::Applied,
            "{application:?}"
        );
        assert!(
            matches!(
                application.units[&ApplyUnit::Capabilities],
                UnitApplication::Failed { .. }
            ),
            "{application:?}"
        );
        assert_eq!(issue459_profile_ids(&runtime), ["balanced", "deep"]);
        assert!(
            runtime
                .model_view()
                .unavailable
                .as_deref()
                .unwrap()
                .contains("fast")
        );
        assert!(Arc::ptr_eq(
            &tools,
            runtime.runtime_resources().capability().tool_registry()
        ));
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

        // A newer explicit selection recaptures; the Capability change retries.
        fixture
            .manager
            .set_model(&id, issue459_pinned(Some("deep")))
            .await
            .unwrap();
        let retried = applied(&fixture, 0).await;
        assert_eq!(
            retried.units[&ApplyUnit::Capabilities],
            UnitApplication::Applied,
            "{retried:?}"
        );
        assert!(
            runtime
                .runtime_resources()
                .capability()
                .tool_registry()
                .definitions()
                .iter()
                .all(|tool| tool.name != "job_status")
        );
        assert_eq!(
            runtime.model_view().configured,
            issue459_pinned(Some("deep"))
        );
        assert!(runtime.model_view().unavailable.is_none());
        assert_eq!(
            runtime
                .runtime_resources()
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
        fixture.close().await;
    }))
    .await;
}

/// Issue #459: nothing prepared against older authority can restore it.
/// Instructions committed before a publication that removes the pinned
/// `fast` survive it, which keeps the selection explicitly unavailable. A
/// generation overtaken while parked before its fence never becomes visible:
/// only the newer one publishes and commits.
#[allow(clippy::too_many_lines)] // two fencing cases share one ledger
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue459_older_candidates_and_overtaken_generations_never_restore_authority() {
    Box::pin(bounded(async {
        let fixture = Fixture::with_session_count(None, 1).await;
        let id = fixture.sessions[0].id.clone();
        fixture.manager.load(&id, None).await.unwrap();
        let runtime = fixture.manager.configuration_runtime(&id).unwrap();
        let probe = fixture
            .manager
            .probe(&fixture.sessions[0].active_conversation_id);
        issue459_pin_fast(&fixture).await;
        let root_instructions =
            || guidance(runtime.runtime_resources().project_context_files()).to_owned();

        write_guidance(&fixture, "issue459 first instructions".into()).await;
        applied(&fixture, 0).await;
        let revision = runtime.runtime_resources().revision();
        issue459_edit(&fixture, |document| {
            document["models"]["local/a"]["profiles"]
                .as_table_mut()
                .unwrap()
                .remove("fast")
                .unwrap();
        })
        .await;
        let application = applied(&fixture, 0).await;
        assert_eq!(
            application.units[&ApplyUnit::Provider],
            UnitApplication::Applied,
            "{application:?}"
        );
        assert_eq!(issue459_profile_ids(&runtime), ["balanced", "deep"]);
        assert!(runtime.runtime_resources().revision() > revision);
        assert!(
            runtime
                .model_view()
                .unavailable
                .as_deref()
                .unwrap()
                .contains("fast")
        );
        assert_eq!(root_instructions(), "issue459 first instructions");

        // An overtaken generation never becomes visible.
        let profile = |document: &mut toml::Value, name: &str| {
            let mut table = toml::Table::new();
            table.insert("request_params".into(), "{\"temperature\":0.3}".into());
            document["models"]["local/a"]["profiles"]
                .as_table_mut()
                .unwrap()
                .insert(name.into(), table.into());
        };
        probe.before_catalog_publication.arm();
        issue459_edit(&fixture, |document| {
            profile(document, "overtaken");
            set_file_guidance(&fixture, "issue459 overtaken instructions");
        })
        .await;
        probe.before_catalog_publication.entered().await;
        let changes = configuration_changes(&runtime).len();
        issue459_edit(&fixture, |document| {
            document["models"]["local/a"]["profiles"]
                .as_table_mut()
                .unwrap()
                .remove("overtaken")
                .unwrap();
            profile(document, "newest");
            set_file_guidance(&fixture, "issue459 newest instructions");
        })
        .await;
        probe.before_catalog_publication.release();
        applied(&fixture, 0).await;
        assert_eq!(
            issue459_profile_ids(&runtime),
            ["balanced", "deep", "newest"]
        );
        assert_eq!(root_instructions(), "issue459 newest instructions");
        assert_eq!(
            configuration_changes(&runtime).len(),
            changes + 1,
            "only the newest generation committed a change"
        );
        assert!(fixture.provider.request_bodies().is_empty());
        fixture.close().await;
    }))
    .await;
}

/// Issue #459: the explicit Summary policy stays the Session's own across
/// mixed generations, and both native mutation surfaces judge it alike. A
/// generation that removes the primary Profile and changes the instructions
/// keeps the explicit `local/b` Summary configured; recovery keeps it. A
/// generation that removes `local/b` and changes the instructions again
/// makes the selection unavailable on the Summary alone; a primary-only
/// change keeping that Summary is refused identically by `session/setModel`
/// and Runtime Client `model_set`, changing nothing; a complete correction
/// resolves to the same view on both.
#[allow(clippy::too_many_lines)] // one ordered policy contract across two generations
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue459_mixed_generations_keep_the_summary_policy_and_clients_agree() {
    use crate::model::catalog::ModelRef;
    use crate::model::invocation::RequestParams;
    use crate::model::session::{AuxiliaryModelPolicy, AuxiliaryModelView, SessionModelConfig};
    Box::pin(bounded(async {
        let fixture = Fixture::with_session_count(None, 1).await;
        let id = fixture.sessions[0].id.clone();
        fixture.manager.load(&id, None).await.unwrap();
        let runtime = fixture.manager.configuration_runtime(&id).unwrap();
        let host = fixture.manager.configuration_host(&id).unwrap();
        let (client, _) = host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
            .unwrap();
        issue459_pin_fast(&fixture).await;
        let summary_on = |model: &str| AuxiliaryModelPolicy::Explicit {
            model: ModelRef::parse(model).unwrap(),
            profile: None,
            request_params: RequestParams::new(),
            max_output_tokens: Some(256),
        };
        let with_summary = |profile, summary| SessionModelConfig {
            title_model: crate::model::session::AuxiliaryModelPolicy::Session,
            summary_model: summary,
            ..issue459_pinned(profile)
        };
        fixture
            .manager
            .set_model(&id, with_summary(Some("fast"), summary_on("local/b")))
            .await
            .unwrap();
        settled(&fixture, 0).await;
        // The Session owner refuses `config` with a diagnostic naming `subject`;
        // Runtime Client readback and all authority remain unchanged.
        let refused_by_owner = |config: SessionModelConfig, subject: &'static str| {
            let manager = fixture.manager.clone();
            let id = id.clone();
            let runtime = runtime.clone();
            let client = &client;
            async move {
                let view = runtime.model_view();
                let application = manager.configuration_application(&id).unwrap();
                let Err(ModelSelectionError::Model(
                    crate::runtime::conversation_runtime::ModelUpdateError::InvalidConfiguration(
                        diagnostic,
                    ),
                )) = manager.set_model(&id, config.clone()).await
                else {
                    panic!("session/setModel admitted {config:?}")
                };
                let model = super::super::protocol::read_native_model_result(
                    client,
                    client.model_get().unwrap(),
                )
                .await;
                assert!(diagnostic.contains(subject), "{diagnostic}");
                assert_eq!(model, view);
                assert_eq!(runtime.model_view(), view);
                assert_eq!(manager.configuration_application(&id).unwrap(), application);
            }
        };

        // The primary Profile goes; the explicit Summary stays configured.
        issue459_edit(&fixture, |document| {
            document["models"]["local/a"]["profiles"]
                .as_table_mut()
                .unwrap()
                .remove("fast")
                .unwrap();
            set_file_guidance(&fixture, "issue459 summary instructions 1");
        })
        .await;
        let application = applied(&fixture, 0).await;
        assert_eq!(
            application.units[&ApplyUnit::Provider],
            UnitApplication::Applied
        );
        assert_eq!(
            application.units[&ApplyUnit::Instructions],
            UnitApplication::Applied,
            "{application:?}"
        );
        let view = runtime.model_view();
        assert!(view.unavailable.as_deref().unwrap().contains("fast"));
        assert_eq!(view.configured.summary_model, summary_on("local/b"));
        refused_by_owner(with_summary(Some("fast"), summary_on("local/b")), "fast").await;
        let view = fixture
            .manager
            .set_model(&id, with_summary(Some("deep"), summary_on("local/b")))
            .await
            .unwrap()
            .0;
        assert!(view.unavailable.is_none());
        let Some(AuxiliaryModelView::Explicit(summary)) = &view.summary else {
            panic!("explicit summary")
        };
        assert_eq!(
            (summary.model.to_string(), summary.max_output_tokens),
            ("local/b".to_owned(), 256)
        );
        settled(&fixture, 0).await;

        // The Summary's Model goes; the primary stays valid.
        issue459_edit(&fixture, |document| {
            document["models"].as_table_mut().unwrap().remove("local/b");
            set_file_guidance(&fixture, "issue459 summary instructions 2");
        })
        .await;
        // The idle Session commits the prepared instructions by itself.
        let application = applied(&fixture, 0).await;
        assert_eq!(
            application.units[&ApplyUnit::Provider],
            UnitApplication::Applied
        );
        let view = runtime.model_view();
        assert!(view.unavailable.as_deref().unwrap().contains("local/b"));
        assert_eq!(
            view.configured,
            with_summary(Some("deep"), summary_on("local/b"))
        );
        refused_by_owner(with_summary(None, summary_on("local/b")), "local/b").await;

        // A complete correction committed by the owner is visible on both surfaces.
        let corrected = with_summary(None, summary_on("local/a"));
        let view = fixture
            .manager
            .set_model(&id, corrected.clone())
            .await
            .unwrap()
            .0;
        assert!(view.unavailable.is_none());
        applied(&fixture, 0).await;
        assert_eq!(runtime.model_view(), view);
        assert_eq!(
            guidance(runtime.runtime_resources().project_context_files()).to_owned(),
            "issue459 summary instructions 2"
        );
        assert!(fixture.provider.request_bodies().is_empty());
        drop(client);
        fixture.close().await;
    }))
    .await;
}
