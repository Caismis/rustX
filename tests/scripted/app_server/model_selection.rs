//! Live selection commits at the real Session owner. Every race is ordered by
//! a native capture/publication gate; timeouts only bound test liveness.
#![allow(clippy::large_futures)]

use super::*;
use crate::local_runtime::configuration::settings::SourceTarget;
use crate::model::catalog::{ModelProfileId, ModelRef};
use crate::model::session::{SessionModelConfig, SummaryModelPolicy};
use crate::runtime::conversation_runtime::{Gate, ModelUpdateError};
use crate::runtime_client::{EventDelivery, RuntimeClientEvent};

struct SelectionGate {
    entered: Option<tokio::sync::oneshot::Receiver<()>>,
    release: Option<std::sync::mpsc::Sender<()>>,
}

impl SelectionGate {
    fn at(fixture: &Fixture, point: &'static str) -> Self {
        let (entered, waiting) = tokio::sync::oneshot::channel();
        let (release, resume) = std::sync::mpsc::channel();
        fixture
            .manager
            .configuration
            .test_hooks
            .insert(point, move || {
                let _ = entered.send(());
                let _ = resume.recv();
            });
        Self {
            entered: Some(waiting),
            release: Some(release),
        }
    }

    fn prepared(fixture: &Fixture) -> Self {
        Self::at(fixture, "model_selection_prepared")
    }

    async fn entered(&mut self) {
        self.entered.take().unwrap().await.unwrap();
    }
}

impl Drop for SelectionGate {
    fn drop(&mut self) {
        if let Some(release) = self.release.take() {
            let _ = release.send(());
        }
    }
}

fn selection(model: &str) -> SessionModelConfig {
    SessionModelConfig::of(ModelRef::parse(model).unwrap())
}

fn select(
    fixture: &Fixture,
    config: SessionModelConfig,
) -> tokio::task::JoinHandle<Result<crate::model::session::SessionModelView, ModelSelectionError>> {
    let manager = fixture.manager.clone();
    let id = fixture.sessions[0].id.clone();
    tokio::spawn(async move { manager.set_model(&id, config).await.map(|(view, _)| view) })
}

async fn fixture() -> Fixture {
    let fixture = Fixture::with_session_count(None, 1).await;
    // The shared residency fixture deliberately creates catalog-only Sessions.
    // Give this suite the durable resolved intent normal Session creation owns.
    let id = &fixture.sessions[0].id;
    let (revision, mut settings) = fixture.manager.sessions.read_settings(id).await.unwrap();
    settings.model = Some(selection("local/a"));
    fixture
        .manager
        .sessions
        .replace_settings(id, revision, settings)
        .await
        .unwrap();
    fixture.load(0).await.unwrap().unwrap();
    fixture
        .manager
        .reconcile_configuration(&SourceTarget::Workspace {
            directory: fixture.workspaces[0].clone(),
        })
        .await
        .unwrap();
    super::configuration::settled(&fixture, 0).await;
    fixture
}

async fn assert_authority(fixture: &Fixture, expected: &SessionModelConfig) {
    let id = &fixture.sessions[0].id;
    let (_, durable) = fixture.manager.sessions.read_settings(id).await.unwrap();
    assert_eq!(durable.model.as_ref(), Some(expected));
    let reopened =
        crate::local_runtime::session::SessionCatalog::open_existing(&fixture.archive_root)
            .unwrap()
            .unwrap();
    assert_eq!(
        reopened.lineage(id, None).unwrap().1.model.as_ref(),
        Some(expected)
    );
    let retained = fixture
        .manager
        .sessions
        .configuration_bindings
        .lock()
        .unwrap()[id]
        .clone();
    assert_eq!(retained.session_model(), expected);
    let runtime = fixture.manager.configuration_runtime(id).unwrap();
    assert_eq!(runtime.model_view().configured, *expected);
    assert_eq!(
        crate::local_runtime::configuration::settings::EffectiveConfiguration::adopted(&runtime)
            .unwrap()
            .adopted_binding,
        retained.binding_revision
    );
    let projected = fixture
        .manager
        .configuration_host(id)
        .unwrap()
        .checkpoint()
        .unwrap()
        .snapshot;
    assert_eq!(projected.model.unwrap().configured, *expected);
}

fn assert_conflict(result: &Result<crate::model::session::SessionModelView, ModelSelectionError>) {
    assert!(
        matches!(
            result,
            Err(ModelSelectionError::Model(ModelUpdateError::Conflict))
        ),
        "{result:?}"
    );
}

async fn gate_entered(gate: &Arc<Gate>) {
    let gate = gate.clone();
    tokio::task::spawn_blocking(move || gate.wait_entered())
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t13_t15_catalog_selection_resolves_original_large_identities_at_exact_generation() {
    bounded(async {
        let f = fixture().await;
        let id = &f.sessions[0].id;
        let runtime = f.manager.configuration_runtime(id).unwrap();
        let mut original = selection("local/a");
        let summary = SummaryModelPolicy::Explicit {
            model: ModelRef::parse("local/b").unwrap(),
            profile: None,
            request_params: serde_json::Map::default(),
            max_output_tokens: Some(128),
        };
        original.summary_model = summary.clone();
        f.manager.set_model(id, original).await.unwrap();
        let before =
            crate::local_runtime::configuration::settings::EffectiveConfiguration::adopted(
                &runtime,
            )
            .unwrap()
            .adopted_binding;
        let source = f
            .manager
            .source_settings(&SourceTarget::User)
            .await
            .unwrap();
        let mut document: toml::Value =
            toml::from_str(&std::fs::read_to_string(&source.user.path).unwrap()).unwrap();
        let enormous_model = format!("local/z{}", "m".repeat(160_000));
        let enormous_profile = format!("profile-{}", "p".repeat(160_000));
        let mut model = document["models"]["local/b"].clone();
        // Profile-owned request parameters cannot coexist with model-level parameters.
        model.as_table_mut().unwrap().remove("request_params");
        model.as_table_mut().unwrap().insert(
            "profiles".into(),
            toml::Value::Table(
                [(
                    enormous_profile.clone(),
                    toml::Value::Table(toml::map::Map::default()),
                )]
                .into_iter()
                .collect(),
            ),
        );
        model
            .as_table_mut()
            .unwrap()
            .insert("default_profile".into(), enormous_profile.clone().into());
        document["models"]
            .as_table_mut()
            .unwrap()
            .insert(enormous_model.clone(), model);
        let encoded = toml::to_string(&document).unwrap();
        assert!(
            encoded.len() <= 1_048_576,
            "fixture respects the native configuration source limit: {} encoded bytes",
            encoded.len()
        );
        std::fs::write(source.user.path, encoded).unwrap();
        f.manager
            .reconcile_configuration(&SourceTarget::User)
            .await
            .unwrap();
        super::configuration::settled(&f, 0).await;
        // The independent Summary may select the same large catalog entry.
        // Its configuration fits admission; the complete resolved primary +
        // Summary view is genuinely larger than a transport record.
        let summary = SummaryModelPolicy::Explicit {
            model: ModelRef::parse(&enormous_model).unwrap(),
            profile: Some(ModelProfileId::parse(&enormous_profile).unwrap()),
            request_params: serde_json::Map::default(),
            max_output_tokens: Some(128),
        };
        let mut retained = runtime.model_view().configured;
        retained.summary_model = summary.clone();
        f.manager.set_model(id, retained).await.unwrap();
        let binding =
            crate::local_runtime::configuration::settings::EffectiveConfiguration::adopted(
                &runtime,
            )
            .unwrap()
            .adopted_binding;
        assert_ne!(
            before,
            binding,
            "{:?}",
            f.manager.configuration_application(id)
        );
        let catalog = runtime.model_read_source().catalog_source();
        let model_position = (0..catalog.model_count() as u64)
            .find(|position| catalog.model_at(*position).unwrap().model.as_str() == enormous_model)
            .unwrap();
        let choice = crate::model::session::CatalogModelSelection {
            binding_revision: binding,
            model_position,
            profile_position: Some(0),
        };
        let revision = f.manager.sessions.read_settings(id).await.unwrap().0;
        let stale = crate::model::session::CatalogModelSelection {
            binding_revision: before,
            ..choice.clone()
        };
        assert!(matches!(
            f.manager.select_catalog_model(id, stale).await,
            Err(ModelSelectionError::Model(ModelUpdateError::Conflict))
        ));
        assert_eq!(
            f.manager.sessions.read_settings(id).await.unwrap().0,
            revision
        );
        assert!(serde_json::to_vec(&choice).unwrap().len() < 256);
        let (_, committed_revision) = f.manager.select_catalog_model(id, choice).await.unwrap();
        assert!(committed_revision > revision);
        let model_view = runtime.model_view();
        assert!(
            serde_json::to_vec(&model_view).unwrap().len() > 1_048_576,
            "resolved native identities and independent Summary exceed one complete record"
        );
        assert!(
            crate::runtime_client::checkpoint::bounded_bytes(&model_view, 1_048_576).is_none(),
            "native model body exceeds the complete transport record budget"
        );
        let configured = model_view.configured;
        assert_eq!(configured.summary_model, summary);
        assert_eq!(configured.model.as_str(), enormous_model);
        assert_eq!(configured.profile.unwrap().as_str(), enormous_profile);
        let (_, durable) = f.manager.sessions.read_settings(id).await.unwrap();
        assert_eq!(
            durable.model.as_ref().unwrap().model.as_str(),
            enormous_model
        );
        let checkpoint = f
            .manager
            .configuration_host(id)
            .unwrap()
            .checkpoint()
            .unwrap();
        assert!(checkpoint.snapshot.model.is_none());
        assert_eq!(
            checkpoint.snapshot.model_status,
            Some(crate::runtime_client::snapshot::RuntimeClientModelStatus {
                available: true,
                body_deferred: true,
                binding_revision: Some(
                    crate::local_runtime::configuration::settings::EffectiveConfiguration::adopted(
                        &runtime
                    )
                    .unwrap()
                    .adopted_binding
                )
            })
        );
        assert!(crate::runtime_client::checkpoint::bounded_bytes(&checkpoint, 786_432).is_some());
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn r04_manager_selection_and_attempt_admission_have_both_complete_orders() {
    bounded(async {
        for selection_first in [true, false] {
            let f = fixture().await;
            let id = &f.sessions[0].id;
            let runtime = f.manager.configuration_runtime(id).unwrap();
            let host = f.manager.configuration_host(id).unwrap();
            let source = f
                .manager
                .source_settings(&SourceTarget::User)
                .await
                .unwrap();
            let mut document: toml::Value =
                toml::from_str(&std::fs::read_to_string(&source.user.path).unwrap()).unwrap();
            let model = document["models"]["local/b"].as_table_mut().unwrap();
            model.remove("request_params");
            model.insert("default_profile".into(), "low".into());
            model.insert(
                "profiles".into(),
                toml::Value::try_from(serde_json::json!({
                    "low": {"request_params": "{\"temperature\":0.9}"}
                }))
                .unwrap(),
            );
            std::fs::write(source.user.path, toml::to_string(&document).unwrap()).unwrap();
            f.manager
                .reconcile_configuration(&SourceTarget::User)
                .await
                .unwrap();
            super::configuration::settled(&f, 0).await;
            let original = SessionModelConfig {
                request_params: serde_json::from_value(
                    serde_json::json!({"temperature": 0.1, "seed": 7}),
                )
                .unwrap(),
                max_output_tokens: Some(333),
                summary_model: SummaryModelPolicy::Explicit {
                    model: ModelRef::parse("local/a").unwrap(),
                    profile: None,
                    request_params: serde_json::from_value(serde_json::json!({"temperature": 0.2}))
                        .unwrap(),
                    max_output_tokens: Some(111),
                },
                ..selection("local/a")
            };
            f.manager.set_model(id, original).await.unwrap();
            super::configuration::settled(&f, 0).await;
            let old_invocation = runtime.model_snapshot().view();
            let selected = SessionModelConfig {
                profile: Some(ModelProfileId::new("low")),
                request_params: serde_json::from_value(serde_json::json!({"seed": 17})).unwrap(),
                max_output_tokens: Some(888),
                summary_model: SummaryModelPolicy::Explicit {
                    model: ModelRef::parse("local/a").unwrap(),
                    profile: None,
                    request_params: serde_json::from_value(serde_json::json!({"temperature": 0.6}))
                        .unwrap(),
                    max_output_tokens: Some(222),
                },
                ..selection("local/b")
            };
            let resources = runtime.runtime_resources();
            let binding =
                crate::local_runtime::configuration::settings::EffectiveConfiguration::adopted(
                    &runtime,
                )
                .unwrap()
                .adopted_binding;
            let settings_revision = f.manager.sessions.read_settings(id).await.unwrap().0;
            let (attachment, _) = host
                .attach_read_only(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
                .unwrap();
            let checkpoint = host.checkpoint().unwrap();
            let (_, before_cursor) = (checkpoint.snapshot, checkpoint.cursor);
            let observations = attachment.subscribe_events(before_cursor).unwrap();
            let contention = Arc::new(tokio::sync::Notify::new());
            let held = Arc::new(Gate::default());
            let held_release = held.arm_scoped();
            let settlement = runtime.settlement_signal();
            let selection;
            let admission_cursor;
            if selection_first {
                let entering = Arc::new(Gate::default());
                let entering_release = entering.arm_scoped();
                runtime.install_admission_gate(entering.clone());
                runtime.install_model_selection_race_probe(
                    Some(contention.clone()),
                    None,
                    None,
                    Some(held.clone()),
                );
                runtime.submit_inbound(input("request-A")).unwrap();
                gate_entered(&entering).await;
                selection = select(&f, selected.clone());
                gate_entered(&held).await;
                // Selection owns the coordinator; the released admission must
                // actually observe WouldBlock before this test releases it.
                drop(entering_release);
                contention.notified().await;
                assert!(!selection.is_finished());
                admission_cursor = None;
            } else {
                let mut prepared = SelectionGate::prepared(&f);
                selection = select(&f, selected.clone());
                prepared.entered().await;
                runtime.install_model_selection_race_probe(
                    None,
                    Some(held.clone()),
                    Some(contention.clone()),
                    None,
                );
                runtime.submit_inbound(input("request-A")).unwrap();
                gate_entered(&held).await;
                // The frozen snapshot has been published under the still-held
                // coordinator lock; no model request has started yet.
                let checkpoint = host.checkpoint().unwrap();
                let (admitted, cursor) = (checkpoint.snapshot, checkpoint.cursor);
                assert_eq!(*admitted.attempt.unwrap().model.unwrap(), old_invocation);
                assert!(f.provider.request_bodies().is_empty());
                admission_cursor = Some(cursor);
                drop(prepared);
                contention.notified().await;
                assert!(!selection.is_finished());
            }
            drop(held_release);
            selection.await.unwrap().unwrap();
            f.gates[0].wait_entered().await;
            let frozen = host.checkpoint().unwrap().snapshot.attempt.unwrap();
            assert_authority(&f, &selected).await;
            assert_eq!(
                f.manager.sessions.read_settings(id).await.unwrap().0,
                settings_revision + 1
            );
            assert_eq!(
                crate::local_runtime::configuration::settings::EffectiveConfiguration::adopted(
                    &runtime
                )
                .unwrap()
                .adopted_binding,
                binding + 1
            );
            assert!(Arc::ptr_eq(&resources, &runtime.runtime_resources()));
            let new_invocation = runtime.model_snapshot().view();
            let expected = if selection_first {
                &new_invocation
            } else {
                &old_invocation
            };
            assert_eq!(frozen.model.as_deref().unwrap(), expected);
            assert_eq!(
                frozen
                    .execution_settings
                    .as_ref()
                    .unwrap()
                    .resource_revision,
                resources.revision()
            );
            let (mut changed, mut started) = (None, None);
            while changed.is_none() || started.is_none() {
                let EventDelivery::Event(event) = observations.next().await else {
                    panic!("live observations")
                };
                for fact in event.event.facts() {
                    match fact {
                        RuntimeClientEvent::SessionModelChanged { model, .. } => {
                            assert_eq!(
                                model.as_deref().expect("finite fixture model").configured,
                                selected
                            );
                            assert!(changed.replace(event.cursor).is_none());
                        }
                        RuntimeClientEvent::AttemptStarted {
                            model, attempt_id, ..
                        } => {
                            assert_eq!(model.as_deref().unwrap(), expected);
                            assert_eq!(Some(attempt_id), frozen.attempt_id.as_ref());
                            assert!(started.replace(event.cursor).is_none());
                        }
                        _ => {}
                    }
                }
            }
            assert!(changed.unwrap() > before_cursor && started.unwrap() > before_cursor);
            if selection_first {
                assert!(changed.unwrap() < started.unwrap());
            } else {
                // AttemptStarted is an execution event emitted after admission
                // unlocks; the admitted snapshot above is the ordering evidence.
                assert!(changed.unwrap() > admission_cursor.unwrap());
            }
            let bodies = f.provider.request_bodies();
            let body: serde_json::Value = serde_json::from_str(&bodies[0]).unwrap();
            assert_eq!(body["model"], if selection_first { "b" } else { "a" });
            assert_eq!(
                body["temperature"],
                expected.primary.request_params["temperature"]
            );
            assert_eq!(body["seed"], expected.primary.request_params["seed"]);
            assert_eq!(
                body["max_completion_tokens"],
                expected.primary.max_output_tokens
            );
            f.gates[0].release();
            settlement.notified().await;
            super::configuration::settled(&f, 0).await;
            runtime
                .submit_inbound(input("next independent admission"))
                .unwrap();
            settlement.notified().await;
            let bodies = f.provider.request_bodies();
            assert_eq!(bodies.len(), 2);
            let body: serde_json::Value = serde_json::from_str(&bodies[1]).unwrap();
            assert_eq!(body["model"], "b");
            assert_eq!(
                body["temperature"],
                new_invocation.primary.request_params["temperature"]
            );
            assert_eq!(body["seed"], new_invocation.primary.request_params["seed"]);
            assert_eq!(
                body["max_completion_tokens"],
                new_invocation.primary.max_output_tokens
            );
            f.close().await;
        }
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn r05_concurrent_prepared_selection_cannot_overwrite_the_winning_commit() {
    bounded(async {
        let f = fixture().await;
        let mut first_gate = SelectionGate::prepared(&f);
        let first = select(&f, selection("local/b"));
        first_gate.entered().await;
        let mut winning = selection("local/a");
        winning
            .request_params
            .insert("seed".into(), serde_json::json!(17));
        let mut second_gate = SelectionGate::prepared(&f);
        let second = select(&f, winning.clone());
        second_gate.entered().await;
        assert!(!first.is_finished() && !second.is_finished());
        drop(second_gate);
        second.await.unwrap().unwrap();
        super::configuration::settled(&f, 0).await;
        let before = f
            .manager
            .sessions
            .read_settings(&f.sessions[0].id)
            .await
            .unwrap();
        drop(first_gate);
        assert_conflict(&first.await.unwrap());
        assert_authority(&f, &winning).await;
        assert_eq!(
            f.manager
                .sessions
                .read_settings(&f.sessions[0].id)
                .await
                .unwrap(),
            before
        );
        assert!(f.provider.request_bodies().is_empty());
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn r06_catalog_publication_rejects_prepared_selection_without_adopting_context() {
    bounded(async {
        let f = fixture().await;
        let id = &f.sessions[0].id;
        let runtime = f.manager.configuration_runtime(id).unwrap();
        let original_instructions = runtime
            .runtime_resources()
            .configuration()
            .unwrap()
            .config
            .agent
            .instructions
            .clone();
        let original_settings = f.manager.sessions.read_settings(id).await.unwrap();
        let mut gate = SelectionGate::prepared(&f);
        let pending = select(&f, selection("local/b"));
        gate.entered().await;
        let source = f
            .manager
            .source_settings(&SourceTarget::User)
            .await
            .unwrap();
        let mut document: toml::Value =
            toml::from_str(&std::fs::read_to_string(&source.user.path).unwrap()).unwrap();
        let mut third = document["models"]["local/b"].clone();
        third["id"] = "c".into();
        document["models"]
            .as_table_mut()
            .unwrap()
            .insert("local/c".into(), third);
        document["agent"].as_table_mut().unwrap().insert(
            "instructions".into(),
            "pending unrelated instructions".into(),
        );
        std::fs::write(source.user.path, toml::to_string(&document).unwrap()).unwrap();
        f.manager
            .reconcile_configuration(&SourceTarget::User)
            .await
            .unwrap();
        let applied = super::configuration::settled(&f, 0).await;
        assert!(applied.candidate.is_some());
        assert!(
            runtime
                .model_catalog()
                .models
                .iter()
                .any(|model| model.model.to_string() == "local/c")
        );
        drop(gate);
        assert_conflict(&pending.await.unwrap());
        assert_eq!(
            f.manager.sessions.read_settings(id).await.unwrap(),
            original_settings
        );
        let selected = selection("local/c");
        f.manager.set_model(id, selected.clone()).await.unwrap();
        super::configuration::settled(&f, 0).await;
        assert_authority(&f, &selected).await;
        assert_eq!(
            runtime
                .runtime_resources()
                .configuration()
                .unwrap()
                .config
                .agent
                .instructions,
            original_instructions
        );
        assert!(
            f.manager
                .configuration_application(id)
                .unwrap()
                .candidate
                .is_some()
        );
        assert!(f.provider.request_bodies().is_empty());
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn r06_policy_publication_at_same_binding_fences_prepared_selection_resources() {
    bounded(async {
        for shared_capacity in [false, true] {
            let f = fixture().await;
            let id = &f.sessions[0].id;
            let runtime = f.manager.configuration_runtime(id).unwrap();
            let before = runtime.runtime_resources();
            let binding = crate::local_runtime::configuration::settings::EffectiveConfiguration::adopted(&runtime).unwrap().adopted_binding;
            let settings = f.manager.sessions.read_settings(id).await.unwrap();
            let mut policy_gate = SelectionGate::at(&f, "configuration_policy_prepared");
            let probe = f.manager.probe(&f.id(0).await);
            probe.before_catalog_publication.arm();
            let path = f.workspaces[0].join("rustx.toml");
            let existing = std::fs::read_to_string(&path).unwrap();
            let policy = if shared_capacity {
                "[subagents]\nmax_concurrent=3\n"
            } else {
                "[model_timeout_policy]\nresponse_start_timeout_ms=12345\nstream_idle_timeout_ms=23456\n"
            };
            std::fs::write(path, format!("{existing}\n{policy}")).unwrap();
            f.manager.reconcile_configuration(&SourceTarget::Workspace { directory: f.workspaces[0].clone() }).await.unwrap();
            policy_gate.entered().await;
            let identity = f.manager.configuration_application(id).unwrap().desired;
            let mut selection_gate = SelectionGate::prepared(&f);
            let pending = select(&f, selection("local/b"));
            selection_gate.entered().await;
            drop(policy_gate);
            probe.before_catalog_publication.entered().await;
            assert_eq!(f.manager.configuration_application(id).unwrap().desired, identity);
            assert_eq!(crate::local_runtime::configuration::settings::EffectiveConfiguration::adopted(&runtime).unwrap().adopted_binding, binding);
            assert!(!Arc::ptr_eq(&runtime.runtime_resources(), &before));
            if shared_capacity {
                assert_eq!(runtime.runtime_resources().revision(), before.revision());
                assert_eq!(crate::local_runtime::configuration::settings::EffectiveConfiguration::adopted(&runtime).unwrap().child_capacity.max_concurrent, 3);
            } else {
                assert_eq!(crate::local_runtime::configuration::settings::EffectiveConfiguration::adopted(&runtime).unwrap().model_timeout.response_start_timeout_ms, 12345);
            }
            drop(selection_gate);
            let result = pending.await.unwrap();
            probe.before_catalog_publication.release();
            assert_conflict(&result);
            super::configuration::settled(&f, 0).await;
            assert_eq!(f.manager.sessions.read_settings(id).await.unwrap(), settings);
            assert_authority(&f, &selection("local/a")).await;
            f.close().await;
        }
    }).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn r05_concurrent_settings_revision_rejects_prepared_selection() {
    bounded(async {
        let f = fixture().await;
        let id = &f.sessions[0].id;
        let mut gate = SelectionGate::prepared(&f);
        let pending = select(&f, selection("local/b"));
        gate.entered().await;
        let (revision, settings) = f.manager.sessions.read_settings(id).await.unwrap();
        f.manager
            .sessions
            .replace_settings(id, revision, settings.clone())
            .await
            .unwrap();
        drop(gate);
        assert_conflict(&pending.await.unwrap());
        assert_eq!(
            f.manager.sessions.read_settings(id).await.unwrap(),
            (revision + 1, settings)
        );
        assert_authority(&f, &selection("local/a")).await;
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn r07_catalog_failure_before_and_after_visibility_preserves_coherent_authority() {
    bounded(async {
        for after_visibility in [false, true] {
            let f = fixture().await;
            let id = &f.sessions[0].id;
            let runtime = f.manager.configuration_runtime(id).unwrap();
            let resources = runtime.runtime_resources();
            let binding =
                crate::local_runtime::configuration::settings::EffectiveConfiguration::adopted(
                    &runtime,
                )
                .unwrap()
                .adopted_binding;
            let (revision, _) = f.manager.sessions.read_settings(id).await.unwrap();
            let host = f.manager.configuration_host(id).unwrap();
            let checkpoint = host.checkpoint().unwrap();
            let (_, cursor) = (checkpoint.snapshot, checkpoint.cursor);
            {
                let catalog = f.manager.sessions.catalog.lock().await;
                if after_visibility {
                    catalog.arm_write_fault_after_rename();
                } else {
                    catalog.arm_write_fault_before_rename();
                }
            }
            let result = f.manager.set_model(id, selection("local/b")).await;
            let Err(ModelSelectionError::Persistence(error)) = result else {
                panic!("{result:?}")
            };
            assert_eq!(error.committed(), after_visibility);
            if after_visibility {
                super::configuration::settled(&f, 0).await;
            }
            let expected = selection(if after_visibility {
                "local/b"
            } else {
                "local/a"
            });
            assert_authority(&f, &expected).await;
            assert_eq!(
                f.manager.sessions.read_settings(id).await.unwrap().0,
                revision + u64::from(after_visibility)
            );
            assert_eq!(
                crate::local_runtime::configuration::settings::EffectiveConfiguration::adopted(
                    &runtime
                )
                .unwrap()
                .adopted_binding,
                binding + u64::from(after_visibility)
            );
            assert!(Arc::ptr_eq(&resources, &runtime.runtime_resources()));
            if !after_visibility {
                assert_eq!(host.checkpoint().unwrap().cursor, cursor);
            }
            assert!(f.provider.request_bodies().is_empty());
            f.close().await;
        }
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn r08_retirement_and_selection_commit_have_both_valid_orders() {
    bounded(async {
        for retirement_first in [true, false] {
            let f = fixture().await;
            let id = &f.sessions[0].id;
            let old_runtime = f.manager.configuration_runtime(id).unwrap();
            let original = f.manager.sessions.read_settings(id).await.unwrap();
            let mut gate = SelectionGate::prepared(&f);
            let pending = select(&f, selection("local/b"));
            gate.entered().await;
            if retirement_first {
                f.manager.unload(&f.id(0).await).await.unwrap();
                assert!(matches!(
                    f.manager.set_model(id, selection("local/a")).await,
                    Err(ModelSelectionError::Model(ModelUpdateError::Inactive))
                ));
                drop(gate);
                let result = pending.await.unwrap();
                assert!(
                    matches!(
                        result,
                        Err(ModelSelectionError::Model(ModelUpdateError::Inactive))
                    ),
                    "{result:?}"
                );
                assert_eq!(
                    f.manager.sessions.read_settings(id).await.unwrap(),
                    original
                );
                assert_eq!(old_runtime.model_view().configured, selection("local/a"));
            } else {
                drop(gate);
                pending.await.unwrap().unwrap();
                super::configuration::settled(&f, 0).await;
                f.manager.unload(&f.id(0).await).await.unwrap();
            }
            f.load(0).await.unwrap().unwrap();
            super::configuration::settled(&f, 0).await;
            assert_authority(
                &f,
                &selection(if retirement_first {
                    "local/a"
                } else {
                    "local/b"
                }),
            )
            .await;
            assert!(f.provider.request_bodies().is_empty());
            f.close().await;
        }
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn r10_native_selection_survives_request_waiter_abort_without_replay() {
    bounded(async {
        let f = fixture().await;
        let id = &f.sessions[0].id;
        let original_settings = f.manager.sessions.read_settings(id).await.unwrap();
        let original_application = f.manager.configuration_application(id).unwrap();
        let host = f.manager.configuration_host(id).unwrap();
        let checkpoint = host.checkpoint().unwrap();
        let (_, original_cursor) = (checkpoint.snapshot, checkpoint.cursor);
        let mut gate = SelectionGate::prepared(&f);
        let pending = select(&f, selection("local/b"));
        gate.entered().await;
        pending.abort();
        assert!(pending.await.unwrap_err().is_cancelled());

        // A fresh attachment successfully reads A while the accepted native B
        // operation still owns its parked commit. This read cannot settle B.
        let (readback, initialized) = host
            .attach_read_only(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
            .unwrap();
        let crate::runtime_client::RuntimeClientResult::Initialized { .. } = initialized else {
            panic!("fresh readback")
        };
        let checkpoint = readback.checkpoint().unwrap();
        let (snapshot, cursor) = (checkpoint.snapshot, checkpoint.cursor);
        assert_eq!(snapshot.model.unwrap().configured, selection("local/a"));
        assert_eq!(cursor, original_cursor);
        let observations = readback.subscribe_events(cursor).unwrap();
        // Even an acknowledged deliberate selection of already explicit A is
        // a true NoOp. It supplies no new revision capable of fencing late B.
        f.manager.set_model(id, selection("local/a")).await.unwrap();
        assert_eq!(
            f.manager.sessions.read_settings(id).await.unwrap(),
            original_settings
        );
        assert_eq!(
            f.manager.configuration_application(id).unwrap(),
            original_application
        );
        assert_eq!(host.checkpoint().unwrap().cursor, original_cursor);
        drop(gate);
        loop {
            let EventDelivery::Event(event) = observations.next().await else {
                panic!("late native selection observation")
            };
            if let Some(model) = event.event.facts().iter().find_map(|fact| match fact {
                RuntimeClientEvent::SessionModelChanged { model, .. } => Some(model),
                _ => None,
            }) {
                assert_eq!(
                    model.as_deref().expect("finite fixture model").configured,
                    selection("local/b")
                );
                assert!(event.cursor > cursor);
                break;
            }
        }
        super::configuration::settled(&f, 0).await;
        assert_authority(&f, &selection("local/b")).await;
        readback.detach();
        let (reconnected, initialized) = host
            .attach_read_only(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
            .unwrap();
        let crate::runtime_client::RuntimeClientResult::Initialized { .. } = initialized else {
            panic!("reconnect")
        };
        let snapshot = reconnected.checkpoint().unwrap().snapshot;
        assert_eq!(snapshot.model.unwrap().configured, selection("local/b"));
        assert_eq!(
            f.manager.sessions.read_settings(id).await.unwrap().0,
            original_settings.0 + 1,
            "readback and reconnect never replay the lost mutation"
        );
        assert!(f.provider.request_bodies().is_empty());
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn f4_later_deliberate_selection_fences_a_prepared_request_after_old_readback() {
    bounded(async {
        let f = fixture().await;
        let id = &f.sessions[0].id;
        let host = f.manager.configuration_host(id).unwrap();
        let initial_revision = f.manager.sessions.read_settings(id).await.unwrap().0;
        let mut gate = SelectionGate::prepared(&f);
        let pending = select(&f, selection("local/b"));
        gate.entered().await;
        let (readback, initialized) = host
            .attach_read_only(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
            .unwrap();
        let crate::runtime_client::RuntimeClientResult::Initialized { .. } = initialized else {
            panic!("old readback")
        };
        let snapshot = readback.checkpoint().unwrap().snapshot;
        assert_eq!(snapshot.model.unwrap().configured, selection("local/a"));
        let mut deliberate = selection("local/a");
        deliberate
            .request_params
            .insert("seed".into(), serde_json::json!(19));
        f.manager.set_model(id, deliberate.clone()).await.unwrap();
        super::configuration::settled(&f, 0).await;
        let settings = f.manager.sessions.read_settings(id).await.unwrap();
        let application = f.manager.configuration_application(id).unwrap();
        let checkpoint = host.checkpoint().unwrap();
        let (_, cursor) = (checkpoint.snapshot, checkpoint.cursor);
        assert_eq!(settings.0, initial_revision + 1);
        drop(gate);
        // Keep the native result for this independent CAS proof; the preceding
        // regression separately proves a dropped RPC waiter does not cancel it.
        assert_conflict(&pending.await.unwrap());
        assert_authority(&f, &deliberate).await;
        assert_eq!(
            f.manager.sessions.read_settings(id).await.unwrap(),
            settings
        );
        assert_eq!(
            f.manager.configuration_application(id).unwrap(),
            application
        );
        assert_eq!(host.checkpoint().unwrap().cursor, cursor);
        readback.detach();
        let (reconnected, initialized) = host
            .attach_read_only(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
            .unwrap();
        let crate::runtime_client::RuntimeClientResult::Initialized { .. } = initialized else {
            panic!("new readback")
        };
        let snapshot = reconnected.checkpoint().unwrap().snapshot;
        assert_eq!(snapshot.model.unwrap().configured, deliberate);
        assert!(f.provider.request_bodies().is_empty());
        f.close().await;
    })
    .await;
}
