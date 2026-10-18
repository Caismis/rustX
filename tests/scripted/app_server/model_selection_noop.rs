//! A repeated explicit selection is inert across every Application lifecycle.
#![allow(clippy::large_futures)]

use super::*;
use crate::local_runtime::configuration::settings::SourceTarget;
use crate::model::catalog::ModelRef;
use crate::model::session::{SessionModelConfig, SummaryModelPolicy};
use crate::runtime::conversation_runtime::Gate;

struct NoopStage {
    label: std::cell::Cell<&'static str>,
    entered: std::cell::Cell<std::time::Instant>,
    started: std::time::Instant,
}

impl NoopStage {
    fn new() -> Self {
        let now = std::time::Instant::now();
        Self {
            label: std::cell::Cell::new("fixture"),
            entered: std::cell::Cell::new(now),
            started: now,
        }
    }

    fn set(&self, label: &'static str) {
        self.label.set(label);
        self.entered.set(std::time::Instant::now());
    }

    fn get(&self) -> &'static str {
        self.label.get()
    }
}

async fn fixture() -> Fixture {
    let f = Fixture::with_session_count(None, 1).await;
    let id = &f.sessions[0].id;
    let (revision, mut settings) = f.manager.sessions.read_settings(id).await.unwrap();
    let mut selection = SessionModelConfig::of(ModelRef::parse("local/a").unwrap());
    selection.request_params.insert("seed".into(), 17.into());
    selection.max_output_tokens = Some(777);
    selection.summary_model = SummaryModelPolicy::Explicit {
        model: ModelRef::parse("local/b").unwrap(),
        profile: None,
        request_params: serde_json::Map::default(),
        max_output_tokens: Some(333),
    };
    settings.model = Some(selection);
    f.manager
        .sessions
        .replace_settings(id, revision, settings)
        .await
        .unwrap();
    f.load(0).await.unwrap().unwrap();
    f.manager
        .reconcile_configuration(&SourceTarget::Workspace {
            directory: f.workspaces[0].clone(),
        })
        .await
        .unwrap();
    super::configuration::settled(&f, 0).await;
    published_activation(&f.manager.configuration_host(id).unwrap()).await;
    f
}

async fn published_activation(host: &crate::runtime_client::RuntimeClientHost) {
    use crate::local_runtime::configuration::application::AdoptionEligibility;
    use crate::runtime_client::{EventDelivery, RuntimeClientCursor, RuntimeClientEvent};
    let (reader, _) = host
        .attach_read_only(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
        .unwrap();
    let subscription = reader
        .subscribe_events(RuntimeClientCursor::new(0))
        .unwrap();
    loop {
        let EventDelivery::Event(event) = subscription.next().await else {
            panic!("retained original activation publication");
        };
        if event.event.facts().iter().any(|fact| match fact {
            RuntimeClientEvent::ConfigurationAdoptionEligibilityChanged { eligibility } => {
                *eligibility == AdoptionEligibility::Eligible
            }
            RuntimeClientEvent::CheckpointAdvanced { checkpoint, .. } => {
                assert_eq!(checkpoint.cursor, event.cursor);
                checkpoint.snapshot.configuration_adoption_eligibility
                    == AdoptionEligibility::Eligible
            }
            _ => false,
        }) {
            break;
        }
    }
    drop(subscription);
    reader.detach();
}

async fn assert_noop(f: &Fixture, stage: Option<&NoopStage>) {
    let mark = |label| {
        if let Some(stage) = stage {
            stage.set(label);
        }
    };
    mark("no-op settings baseline");
    let id = &f.sessions[0].id;
    let runtime = f.manager.configuration_runtime(id).unwrap();
    let host = f.manager.configuration_host(id).unwrap();
    // The caller parks execution at native admission. Checkpoint reads are
    // pure and cannot publish a repair or change the measured cursor.
    let settings = f.manager.sessions.read_settings(id).await.unwrap();
    let selection = settings.1.model.clone().unwrap();
    let catalog = f.archive_root.join("sessions/catalog.json");
    let bytes = std::fs::read(&catalog).unwrap();
    let modified = std::fs::metadata(&catalog).unwrap().modified().unwrap();
    let retained = || {
        let bindings = f.manager.sessions.configuration_bindings.lock().unwrap();
        let binding = &bindings[id];
        (
            std::ptr::from_ref::<crate::local_runtime::configuration::ProspectiveSessionConfig>(
                binding,
            ) as usize,
            binding.binding_revision,
            binding.session_model().clone(),
        )
    };
    let binding = retained();
    let resources = runtime.runtime_resources();
    let view = runtime.model_view();
    let configuration = runtime.configuration_view().unwrap();
    mark("no-op baseline publication");
    host.projection_settled().await;
    mark("no-op parked baseline");
    host.inner.park_projection_worker();
    let checkpoint = host.checkpoint().unwrap();
    let (projection, cursor) = (checkpoint.snapshot, checkpoint.cursor);
    let application = f.manager.configuration_application(id);
    let activity = f.manager.applications.lock().selection_probe(id.as_ref());
    let probe = f.manager.probe(&f.id(0).await);
    let preparations = probe.configuration_preparations.load(Ordering::SeqCst);
    let changes = f.manager.configuration_changes();
    let (reader, _) = host
        .attach_read_only(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
        .unwrap();
    let subscription = reader.subscribe_events(cursor).unwrap();

    mark("no-op model selection");
    assert_eq!(
        f.manager.set_model(id, selection).await.unwrap(),
        (view.clone(), settings.0)
    );
    assert!(matches!(
        subscription.try_next(),
        crate::runtime_client::EventDelivery::Pending
    ));
    drop(subscription);
    reader.detach();

    assert_eq!(
        f.manager.sessions.read_settings(id).await.unwrap(),
        settings
    );
    assert_eq!(std::fs::read(&catalog).unwrap(), bytes);
    assert_eq!(
        std::fs::metadata(&catalog).unwrap().modified().unwrap(),
        modified
    );
    assert_eq!(
        retained(),
        binding,
        "retained allocation and complete selection unchanged"
    );
    assert!(Arc::ptr_eq(&runtime.runtime_resources(), &resources));
    assert_eq!(runtime.model_view(), view);
    assert_eq!(runtime.configuration_view().unwrap(), configuration);
    host.inner.unpark_projection_worker();
    mark("no-op subsequent publication");
    host.projection_settled().await;
    let checkpoint = host.checkpoint().unwrap();
    let (after, after_cursor) = (checkpoint.snapshot, checkpoint.cursor);
    assert_eq!(after.model, projection.model);
    assert_eq!(after.attempt, projection.attempt);
    assert_eq!(after_cursor, cursor, "no runtime observation");
    assert_eq!(f.manager.configuration_application(id), application);
    assert_eq!(
        f.manager.applications.lock().selection_probe(id.as_ref()),
        activity,
        "no new identity, version, pending work, worker start, or Ready Candidate replacement"
    );
    assert_eq!(
        probe.configuration_preparations.load(Ordering::SeqCst),
        preparations
    );
    assert!(
        !changes.has_changed().unwrap(),
        "no Application notification"
    );
}

async fn edit_instructions(f: &Fixture) {
    let path = f.workspaces[0].join("rustx.toml");
    let mut source: toml::Value = toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    source
        .as_table_mut()
        .unwrap()
        .entry("agent")
        .or_insert_with(|| toml::Value::Table(toml::map::Map::default()))
        .as_table_mut()
        .unwrap()
        .insert("instructions".into(), "pending explicit adoption".into());
    std::fs::write(path, toml::to_string(&source).unwrap()).unwrap();
    f.manager
        .reconcile_configuration(&SourceTarget::Workspace {
            directory: f.workspaces[0].clone(),
        })
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn identical_explicit_selection_is_inert_while_idle() {
    bounded(async {
        let f = fixture().await;
        assert_noop(&f, None).await;
        assert_noop(&f, None).await;
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn identical_explicit_selection_is_inert_during_an_attempt() {
    let stage = NoopStage::new();
    let diagnostic_host = Arc::new(Mutex::new(None));
    let work_host = diagnostic_host.clone();
    let mut work = Box::pin(async {
        let f = fixture().await;
        stage.set("idle projection settled");
        let runtime = f.manager.configuration_runtime(&f.sessions[0].id).unwrap();
        let host = f.manager.configuration_host(&f.sessions[0].id).unwrap();
        *work_host.lock().unwrap() = Some(host.clone());
        host.projection_settled().await;
        let baseline = host.checkpoint().unwrap();
        assert_eq!(
            baseline.snapshot.configuration_adoption_eligibility,
            crate::local_runtime::configuration::application::AdoptionEligibility::Eligible
        );
        let (reader, _) = host
            .attach_read_only(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
            .unwrap();
        let subscription = reader.subscribe_events(baseline.cursor).unwrap();
        let execution = Arc::new(Gate::default());
        let execution_release = execution.arm_scoped();
        runtime.install_attempt_execution_gate(execution.clone());
        let after = runtime.settlement_ordinal_for_test();
        runtime.submit_inbound(input("request-A")).unwrap();
        stage.set("native execution gate");
        tokio::task::spawn_blocking(move || execution.wait_entered())
            .await
            .unwrap();
        assert!(runtime.has_current_attempt());
        stage.set("native admitted checkpoint");
        // The positive idle publication was established before admission;
        // the native admitted receipt is serviceable before execution starts.
        let crate::runtime_client::EventDelivery::Event(event) = subscription.try_next() else {
            panic!("original admitted attempt publication");
        };
        assert!(event.cursor > baseline.cursor);
        assert_eq!(
            host.checkpoint().unwrap().snapshot.attempt.unwrap().phase,
            crate::runtime_client::RuntimeClientAttemptPhase::Admitted
        );
        drop(subscription);
        reader.detach();
        drop(host);
        stage.set("assert noop");
        assert_noop(&f, Some(&stage)).await;
        stage.set("provider gate");
        drop(execution_release);
        f.gates[0].wait_entered().await;
        f.gates[0].release();
        stage.set("settlement");
        let receipt = runtime.child_attempt_settlement(after).await;
        assert_eq!(receipt.ordinal, after + 1);
        stage.set("close");
        *work_host.lock().unwrap() = None;
        f.close().await;
    });
    // Keep the actual execution/release guards alive until the deadline's
    // diagnostic is captured. Cancelling first can advance the very owner
    // whose blocked stage this failure must identify.
    let result = tokio::time::timeout(std::time::Duration::from_secs(30), work.as_mut()).await;
    if result.is_err()
        && let Some(host) = diagnostic_host.lock().unwrap().as_ref()
    {
        eprintln!(
            "no-op liveness at {} (stage {:?}, total {:?}; pre-cancellation state): {}",
            stage.get(),
            stage.entered.get().elapsed(),
            stage.started.elapsed(),
            host.inner.projection_source_debug()
        );
    }
    drop(work);
    assert!(result.is_ok(), "test liveness at {}", stage.get());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn identical_explicit_selection_preserves_the_ready_candidate() {
    bounded(async {
        let f = fixture().await;
        edit_instructions(&f).await;
        let ready = super::configuration::settled(&f, 0).await;
        let candidate = ready.candidate.unwrap();
        assert_noop(&f, None).await;
        f.manager
            .adopt_configuration(
                &f.sessions[0].id,
                &candidate.identity,
                candidate.expected_binding,
            )
            .unwrap();
        assert_eq!(
            f.manager
                .configuration_runtime(&f.sessions[0].id)
                .unwrap()
                .runtime_resources()
                .configuration()
                .unwrap()
                .config
                .agent
                .instructions,
            "pending explicit adoption"
        );
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn identical_explicit_selection_is_inert_during_source_publication() {
    bounded(async {
        let f = fixture().await;
        let probe = f.manager.probe(&f.id(0).await);
        probe.before_catalog_publication.arm();
        edit_instructions(&f).await;
        probe.before_catalog_publication.entered().await;
        assert_noop(&f, None).await;
        probe.before_catalog_publication.release();
        assert!(
            super::configuration::settled(&f, 0)
                .await
                .candidate
                .is_some()
        );
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn inherited_current_selection_commits_explicit_intent_then_becomes_inert() {
    bounded(async {
        let f = Fixture::with_session_count(None, 1).await;
        f.load(0).await.unwrap().unwrap();
        let id = &f.sessions[0].id;
        let before = f.manager.sessions.read_settings(id).await.unwrap();
        assert!(before.1.model.is_none());
        let runtime = f.manager.configuration_runtime(id).unwrap();
        let view = runtime.model_view();
        let binding = runtime.configuration_view().unwrap().adopted_binding;
        assert_eq!(
            f.manager
                .set_model(id, view.configured.clone())
                .await
                .unwrap()
                .0,
            view
        );
        let explicit = f.manager.sessions.read_settings(id).await.unwrap();
        assert_eq!(explicit.0, before.0 + 1);
        assert_eq!(explicit.1.model, Some(view.configured));
        // Composition already froze the inherited invocation into the retained
        // runtime binding. Persisting explicit intent changes the catalog, not
        // that identical resolved invocation.
        assert_eq!(
            runtime.configuration_view().unwrap().adopted_binding,
            binding
        );
        published_activation(&f.manager.configuration_host(id).unwrap()).await;
        assert_noop(&f, None).await;
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn identical_explicit_selection_is_inert_after_reload() {
    bounded(async {
        let f = fixture().await;
        f.manager.unload(&f.id(0).await).await.unwrap();
        f.load(0).await.unwrap().unwrap();
        super::configuration::settled(&f, 0).await;
        published_activation(&f.manager.configuration_host(&f.sessions[0].id).unwrap()).await;
        assert_noop(&f, None).await;
        f.close().await;
    })
    .await;
}
