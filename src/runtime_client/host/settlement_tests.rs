use super::*;

struct PublicationPark {
    entered: std::sync::mpsc::Receiver<()>,
    release: std::sync::mpsc::Sender<()>,
}

impl Drop for PublicationPark {
    fn drop(&mut self) {
        let _ = self.release.send(());
    }
}

fn park_publication(inner: &super::super::ClientInner, encoding: bool) -> PublicationPark {
    let (entered_tx, entered) = std::sync::mpsc::channel();
    let (release, release_rx) = std::sync::mpsc::channel();
    let hook: Box<dyn FnOnce() + Send> = Box::new(move || {
        entered_tx.send(()).unwrap();
        let _ = release_rx.recv();
    });
    let slot = if encoding {
        &inner.publication_gate.encoding_hook
    } else {
        &inner.publication_gate.preparation_hook
    };
    *slot.lock().unwrap() = Some(hook);
    PublicationPark { entered, release }
}

async fn wait_signal(receiver: std::sync::mpsc::Receiver<()>) {
    tokio::task::spawn_blocking(move || {
        receiver
            .recv_timeout(BACKGROUND_LIVENESS_GUARD)
            .expect("the actual publication boundary was reached");
    })
    .await
    .unwrap();
}

fn start_settlement_waiter(
    inner: Arc<super::super::ClientInner>,
) -> (tokio::task::JoinHandle<()>, std::sync::mpsc::Receiver<()>) {
    let (contended_tx, contended_rx) = std::sync::mpsc::channel();
    *inner
        .publication_gate
        .settlement_contention_hook
        .lock()
        .unwrap() = Some(Box::new(move || contended_tx.send(()).unwrap()));
    let waiter = tokio::task::spawn_blocking(move || {
        tokio::runtime::Handle::current().block_on(inner.projection_settled());
    });
    (waiter, contended_rx)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn projection_settled_joins_drained_activation_before_eligible_baseline() {
    use crate::local_runtime::configuration::application::AdoptionEligibility;

    let (adapter, fixture) = runtime_only_fixture(Vec::new(), ToolRegistry::new(), None).await;
    let host = RuntimeClientHost::new(RuntimeClientHostConfig {
        runtime: fixture.runtime.clone(),
        replay_limit: None,
    })
    .unwrap();
    let inner = host.weak_inner().upgrade().unwrap();
    let parked = park_publication(&inner, false);
    let cursor = inner.state.lock().unwrap().projection.cursor();
    fixture.runtime.activate();
    parked
        .entered
        .recv_timeout(BACKGROUND_LIVENESS_GUARD)
        .unwrap();
    assert!(inner.pending.settled(), "the real activation was drained");
    {
        let state = inner.state.lock().unwrap();
        assert_eq!(state.projection.cursor(), cursor);
        assert_eq!(
            state
                .projection
                .snapshot_ref()
                .configuration_adoption_eligibility,
            AdoptionEligibility::Unavailable
        );
        assert!(state.projection.read_domain_cut().is_none());
        state.projection.validate_source_prefix().unwrap();
    }
    let (waiter, contended) = start_settlement_waiter(inner.clone());
    wait_signal(contended).await;
    drop(parked);
    waiter.await.unwrap();
    let checkpoint = host.checkpoint().unwrap();
    assert_eq!(
        checkpoint.snapshot.configuration_adoption_eligibility,
        AdoptionEligibility::Eligible
    );
    assert!(checkpoint.cursor > cursor);
    assert!(adapter.requests().is_empty());
    fixture.runtime.shutdown().await.unwrap();
    inner.drain_projection().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn projection_settled_joins_encoding_before_model_baseline() {
    let (adapter, fixture) = runtime_only_fixture(Vec::new(), ToolRegistry::new(), None).await;
    let host = RuntimeClientHost::new(RuntimeClientHostConfig {
        runtime: fixture.runtime.clone(),
        replay_limit: None,
    })
    .unwrap();
    fixture.runtime.activate();
    host.projection_settled().await;
    let inner = host.weak_inner().upgrade().unwrap();
    inner.park_projection_worker();
    let cursor = inner.state.lock().unwrap().projection.cursor();
    let config = marked_model_config(&fixture.runtime, "settlement-marker");
    let parked = park_publication(&inner, true);
    let mutation_host = host.clone();
    let mutation = tokio::task::spawn_blocking(move || mutation_host.model_set(config).unwrap());
    parked
        .entered
        .recv_timeout(BACKGROUND_LIVENESS_GUARD)
        .unwrap();
    assert!(inner.observation_preparation.try_lock().is_ok());
    assert_eq!(inner.state.lock().unwrap().projection.cursor(), cursor);
    let (waiter, contended) = start_settlement_waiter(inner.clone());
    wait_signal(contended).await;
    drop(parked);
    mutation.await.unwrap();
    waiter.await.unwrap();
    let checkpoint = host.checkpoint().unwrap();
    assert!(checkpoint.cursor > cursor);
    assert_eq!(
        checkpoint
            .snapshot
            .model
            .as_ref()
            .unwrap()
            .configured
            .request_params
            .get("settlement-marker"),
        Some(&serde_json::json!("changed"))
    );
    assert!(adapter.requests().is_empty());
    inner.unpark_projection_worker();
    fixture.runtime.shutdown().await.unwrap();
    inner.drain_projection().await.unwrap();
}
