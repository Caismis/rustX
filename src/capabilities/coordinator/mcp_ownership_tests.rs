//! Physical candidate ownership must settle even when its external holder
//! retains the rejected or unadopted configuration beyond Conversation drain.
use super::*;
use crate::tools::mcp::fixture::{FixtureServer, serve_if_fixture_mode};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn drain_settles_retained_candidate_and_late_generation_registration() {
    if serve_if_fixture_mode(FixtureServer::with_list_changed()).await {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let (coordinator, id) = super::mcp_race_tests::coordinator_with_fixture(
        &dir,
        "ownership",
        "capabilities::coordinator::mcp_ownership_tests::drain_settles_retained_candidate_and_late_generation_registration",
    );
    let lifecycle = ConversationLifecycle::new();
    *coordinator.inner.preparation_lifecycle.lock().unwrap() = Some(lifecycle.clone());
    let initial = coordinator.prepare_candidate().await.unwrap();
    coordinator.commit(initial).unwrap();
    assert!(coordinator.claim_conversation_runtime(&lifecycle).is_some());
    assert!(lifecycle.activate());

    let retained = coordinator.prepare_candidate().await.unwrap();
    assert!(!retained.mcp_runtimes.is_empty());
    let connection = retained.mcp_runtimes[0].connection().clone();
    assert!(
        lifecycle.idle_epoch().is_some(),
        "ready resources allow adoption"
    );

    // A connect owner admitted before drain may return after the ownership
    // scan. Registration must observe the closed fence, not escape that scan.
    let late_admission = lifecycle.try_enter_preparation().unwrap();
    let binding = coordinator
        .inner
        .resource_inputs
        .lock()
        .unwrap()
        .mcp_servers[&id]
        .clone();
    assert!(lifecycle.begin_drain());
    coordinator.cancel_conversation_preparation();
    let late = McpRuntimeGeneration::from_connection(
        id.clone(),
        crate::tools::mcp::McpConnection::lazy(
            id,
            binding,
            coordinator.inner.workspace.clone(),
            coordinator.inner.mcp_invalidation.clone(),
            coordinator.inner.preparation_cancellation.clone(),
        ),
        Some(late_admission),
        tokio::runtime::Handle::current(),
        &coordinator.inner.mcp_retirements,
    );
    tokio::time::timeout(
        std::time::Duration::from_secs(15),
        lifecycle.wait_for_no_admissions(),
    )
    .await
    .expect("retained external candidates cannot hold drain open");
    coordinator.drain_conversation_owned().await.unwrap();
    assert!(connection.acquire().await.is_err());
    assert!(late.binding().acquire_lease().is_none());
    assert_eq!(coordinator.pending_mcp_retirements(), 0);
    assert!(lifecycle.mark_quiescent());
    drop(retained);
    drop(late);
}
