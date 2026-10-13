//! Live selection commits at the real Session owner. Every race is ordered by
//! a native capture/publication gate; timeouts only bound test liveness.
#![allow(clippy::large_futures)]

use super::*;
use crate::local_runtime::configuration::settings::SourceTarget;
use crate::model::catalog::ModelRef;
use crate::model::session::SessionModelConfig;
use crate::runtime::conversation_runtime::ModelUpdateError;

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
    tokio::spawn(async move { manager.set_model(&id, config).await })
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
        runtime.configuration_view().unwrap().adopted_binding,
        retained.binding_revision
    );
    let projected = fixture
        .manager
        .configuration_host(id)
        .unwrap()
        .snapshot()
        .unwrap()
        .0;
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn r04_manager_selection_and_attempt_admission_have_both_complete_orders() {
    bounded(async {
        for selection_first in [true, false] {
            let f = fixture().await;
            let id = &f.sessions[0].id;
            let runtime = f.manager.configuration_runtime(id).unwrap();
            let host = f.manager.configuration_host(id).unwrap();
            let selected = selection("local/b");
            if selection_first {
                f.manager.set_model(id, selected.clone()).await.unwrap();
                super::configuration::settled(&f, 0).await;
            }
            let settlement = runtime.settlement_signal();
            runtime.submit_inbound(input("request-A")).unwrap();
            f.gates[0].wait_entered().await;
            let frozen = host.snapshot().unwrap().0.attempt.unwrap();
            if !selection_first {
                f.manager.set_model(id, selected.clone()).await.unwrap();
            }
            assert_authority(&f, &selected).await;
            let expected = if selection_first {
                "local/b"
            } else {
                "local/a"
            };
            assert_eq!(
                frozen.model.as_ref().unwrap().primary.model.to_string(),
                expected
            );
            assert_eq!(
                host.snapshot().unwrap().0.attempt.unwrap().model,
                frozen.model
            );
            let bodies = f.provider.request_bodies();
            let body: serde_json::Value = serde_json::from_str(&bodies[0]).unwrap();
            assert_eq!(body["model"], if selection_first { "b" } else { "a" });
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
            .source_settings(&SourceTarget::User, None)
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
            let binding = runtime.configuration_view().unwrap().adopted_binding;
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
            assert_eq!(runtime.configuration_view().unwrap().adopted_binding, binding);
            assert!(!Arc::ptr_eq(&runtime.runtime_resources(), &before));
            if shared_capacity {
                assert_eq!(runtime.runtime_resources().revision(), before.revision());
                assert_eq!(runtime.configuration_view().unwrap().child_capacity.max_concurrent, 3);
            } else {
                assert_eq!(runtime.configuration_view().unwrap().model_timeout.response_start_timeout_ms, 12345);
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
            let binding = runtime.configuration_view().unwrap().adopted_binding;
            let (revision, _) = f.manager.sessions.read_settings(id).await.unwrap();
            let host = f.manager.configuration_host(id).unwrap();
            let (_, cursor) = host.snapshot().unwrap();
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
                runtime.configuration_view().unwrap().adopted_binding,
                binding + u64::from(after_visibility)
            );
            assert!(Arc::ptr_eq(&resources, &runtime.runtime_resources()));
            if !after_visibility {
                assert_eq!(host.snapshot().unwrap().1, cursor);
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
                assert!(
                    matches!(
                        old_runtime.model_set(selection("local/a")),
                        Err(ModelUpdateError::Inactive)
                    ),
                    "an unchanged selection cannot acknowledge a retired runtime"
                );
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
        let (revision, _) = f.manager.sessions.read_settings(id).await.unwrap();
        let mut changes = f.manager.configuration_changes();
        let mut gate = SelectionGate::prepared(&f);
        let pending = select(&f, selection("local/b"));
        gate.entered().await;
        pending.abort();
        assert!(pending.await.unwrap_err().is_cancelled());
        drop(gate);
        loop {
            if f.manager.sessions.read_settings(id).await.unwrap().1.model
                == Some(selection("local/b"))
            {
                break;
            }
            changes.changed().await.unwrap();
        }
        super::configuration::settled(&f, 0).await;
        assert_authority(&f, &selection("local/b")).await;
        assert_eq!(
            f.manager.sessions.read_settings(id).await.unwrap().0,
            revision + 1
        );
        assert!(f.provider.request_bodies().is_empty());
        f.close().await;
    })
    .await;
}
