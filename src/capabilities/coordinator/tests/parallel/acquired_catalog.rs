//! The startup listing phase never owns another physical acquisition.
use super::*;
use crate::capabilities::CapabilityPreparationError;
use crate::tools::mcp::McpRuntimeGeneration;
use crate::tools::mcp::fixture::fixture_spawn_args;
use crate::tools::mcp::fixture::recovery::{
    RecoveryControl, RecoveryScript, serve_if_recovery_fixture_mode,
};
use crate::tools::mcp::test_sync::{CloseProbe, ConnectOwnershipPause};

struct ReleasePause(Arc<ConnectOwnershipPause>);
impl Drop for ReleasePause {
    fn drop(&mut self) {
        self.0.release();
    }
}

struct ReleaseClose(Arc<CloseProbe>);
impl Drop for ReleaseClose {
    fn drop(&mut self) {
        self.0.release();
        self.0.release_before_failure_publication();
    }
}

struct StdioPreparation {
    coordinator: CapabilityCoordinator,
    lifecycle: ConversationLifecycle,
    id: McpServerId,
    generation: McpRuntimeGeneration,
    control: RecoveryControl,
    supervisor_pid: nix::unistd::Pid,
}

async fn stdio_preparation(dir: &tempfile::TempDir, test: &str) -> StdioPreparation {
    let (coordinator, lifecycle) = coordinator(dir, &[], 0);
    let id = McpServerId::new("acquired-startup");
    let control = RecoveryControl::new(dir.path());
    let binding = McpServerBinding {
        credentials: crate::credentials::SourceCredentials::default(),
        transport: McpTransportConfig::Stdio {
            program: std::env::current_exe().unwrap().display().to_string(),
            args: fixture_spawn_args(test),
            cwd: None,
            environment: control.environment(&RecoveryScript::default()),
        },
        policy: crate::tools::types::ToolInvocationPolicy::default(),
    };
    let pause = Arc::new(ConnectOwnershipPause::default());
    let _release = ReleasePause(pause.clone());
    coordinator.install_connect_ownership_pause(pause.clone());
    let owner = coordinator.clone();
    let server = id.clone();
    let connect = tokio::spawn(async move {
        owner
            .inner
            .mcp_owner
            .connect_conversation_owned(&server, &binding, None)
            .await
    });
    tokio::time::timeout(WAIT, pause.wait_entered())
        .await
        .unwrap();
    let supervisor_pid = nix::unistd::Pid::from_raw(
        i32::try_from(
            pause
                .supervisor_pid()
                .expect("the physical process is owned"),
        )
        .unwrap(),
    );
    pause.release();
    let generation = tokio::time::timeout(WAIT, connect)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    *coordinator.inner.connect_ownership_pause.lock().unwrap() = None;
    assert_eq!(control.established_generations(), 1);
    assert_eq!(control.catalog_requests(1), 0);
    StdioPreparation {
        coordinator,
        lifecycle,
        id,
        generation,
        control,
        supervisor_pid,
    }
}

fn rejected_preparation(
    coordinator: CapabilityCoordinator,
    id: McpServerId,
    generation: McpRuntimeGeneration,
) -> tokio::task::JoinHandle<
    Result<super::super::PreparedCapabilityCandidate, CapabilityPreparationError>,
> {
    tokio::spawn(async move {
        coordinator
            .prepare_owned(None, move |coordinator, cancellation| async move {
                let mut outcomes = coordinator
                    .inner
                    .mcp_owner
                    .prepare_sources([generation], &cancellation, |generation| {
                        coordinator.inner.mcp_owner.prepare_mcp_generation(
                            &id,
                            generation,
                            Some(&cancellation),
                        )
                    })
                    .await;
                match outcomes.pop().unwrap() {
                    Ok(Err(reason)) => Err(CapabilityPreparationError::Mcp(reason)),
                    Ok(Ok((_, generation, _))) | Err(generation) => {
                        generation.retire_and_close().await;
                        panic!("the acquired startup transport must not produce a ready candidate")
                    }
                }
            })
            .await
    })
}

fn assert_process_reaped(pid: nix::unistd::Pid) {
    assert_eq!(
        nix::sys::signal::kill(pid, None),
        Err(nix::errno::Errno::ESRCH)
    );
}

fn assert_no_tool_effects(control: &RecoveryControl) {
    assert!(control.journal_entries().iter().all(|entry| {
        !entry.starts_with(crate::tools::mcp::fixture::recovery::JOURNAL_CALL_PREFIX)
    }));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn lost_transport_after_first_acquire_never_starts_another_startup_generation() {
    if serve_if_recovery_fixture_mode().await {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let StdioPreparation { coordinator, lifecycle, id, generation, control, supervisor_pid } = stdio_preparation(
        &dir,
        "capabilities::coordinator::parallel_tests::acquired_catalog::lost_transport_after_first_acquire_never_starts_another_startup_generation",
    ).await;
    let connection = generation.connection().clone();
    let pause = Arc::new(ConnectOwnershipPause::default());
    let _release = ReleasePause(pause.clone());
    connection.pause_acquired_discovery(pause.clone());
    let worker = rejected_preparation(coordinator.clone(), id.clone(), generation);
    tokio::time::timeout(WAIT, pause.wait_entered())
        .await
        .unwrap();
    assert_eq!(
        coordinator
            .inner
            .source_preparation_slots
            .available_permits(),
        3
    );
    connection.note_published_transport_loss_for_test("lost after startup acquire");
    coordinator.inner.mcp_invalidation.lock().advance(&id);
    pause.release();
    let error = tokio::time::timeout(WAIT, worker)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("unavailable before catalog discovery")
    );
    assert_eq!(connection.established_count().await, 1);
    assert_eq!(control.established_generations(), 1);
    assert_eq!(control.catalog_requests(1), 0);
    assert_no_tool_effects(&control);
    assert!(connection.catalog_snapshot().is_none());
    assert!(lifecycle.begin_drain());
    tokio::time::timeout(WAIT, lifecycle.wait_for_no_admissions())
        .await
        .unwrap();
    coordinator.drain_conversation_owned().await.unwrap();
    assert_eq!(coordinator.pending_mcp_retirements(), 0);
    assert_eq!(
        coordinator
            .inner
            .source_preparation_slots
            .available_permits(),
        4
    );
    assert_process_reaped(supervisor_pid);
    assert!(lifecycle.mark_quiescent());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn invalidation_after_first_acquire_rejects_the_exact_transport_catalog() {
    if serve_if_recovery_fixture_mode().await {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let StdioPreparation { coordinator, lifecycle, id, generation, control, supervisor_pid } = stdio_preparation(
        &dir,
        "capabilities::coordinator::parallel_tests::acquired_catalog::invalidation_after_first_acquire_rejects_the_exact_transport_catalog",
    ).await;
    let connection = generation.connection().clone();
    let pause = Arc::new(ConnectOwnershipPause::default());
    let _release = ReleasePause(pause.clone());
    connection.pause_acquired_discovery(pause.clone());
    let worker = rejected_preparation(coordinator.clone(), id.clone(), generation);
    tokio::time::timeout(WAIT, pause.wait_entered())
        .await
        .unwrap();
    coordinator.inner.mcp_invalidation.lock().advance(&id);
    pause.release();
    let error = tokio::time::timeout(WAIT, worker)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("catalog changed during discovery")
    );
    assert_eq!(connection.established_count().await, 1);
    assert_eq!(control.established_generations(), 1);
    assert_eq!(control.catalog_requests(1), 1);
    assert_no_tool_effects(&control);
    assert!(lifecycle.begin_drain());
    tokio::time::timeout(WAIT, lifecycle.wait_for_no_admissions())
        .await
        .unwrap();
    coordinator.drain_conversation_owned().await.unwrap();
    assert_eq!(coordinator.pending_mcp_retirements(), 0);
    assert_eq!(
        coordinator
            .inner
            .source_preparation_slots
            .available_permits(),
        4
    );
    assert_process_reaped(supervisor_pid);
    assert!(lifecycle.mark_quiescent());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancelled_acquired_discovery_holds_the_slot_until_the_stdio_owner_settles() {
    if serve_if_recovery_fixture_mode().await {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let StdioPreparation { coordinator, lifecycle, id, generation, control, supervisor_pid } = stdio_preparation(
        &dir,
        "capabilities::coordinator::parallel_tests::acquired_catalog::cancelled_acquired_discovery_holds_the_slot_until_the_stdio_owner_settles",
    ).await;
    let connection = generation.connection().clone();
    let pause = Arc::new(ConnectOwnershipPause::default());
    let _release = ReleasePause(pause.clone());
    connection.pause_acquired_discovery(pause.clone());
    let close = Arc::new(CloseProbe::parking());
    let _release_close = ReleaseClose(close.clone());
    connection
        .published_runtime()
        .unwrap()
        .install_close_probe(close.clone());
    let caller = rejected_preparation(coordinator.clone(), id, generation);
    tokio::time::timeout(WAIT, pause.wait_entered())
        .await
        .unwrap();
    connection.note_published_transport_loss_for_test("lost before startup cancellation");
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    tokio::time::timeout(WAIT, close.wait_entered())
        .await
        .unwrap();
    assert!(lifecycle.begin_drain());
    assert!(!lifecycle.mark_quiescent());
    assert_eq!(coordinator.pending_mcp_retirements(), 1);
    assert_eq!(
        coordinator
            .inner
            .source_preparation_slots
            .available_permits(),
        3
    );
    assert_eq!(nix::sys::signal::kill(supervisor_pid, None), Ok(()));
    assert_eq!(control.established_generations(), 1);
    assert_eq!(control.catalog_requests(1), 0);
    close.release();
    tokio::time::timeout(WAIT, lifecycle.wait_for_no_admissions())
        .await
        .unwrap();
    coordinator.drain_conversation_owned().await.unwrap();
    pause.release(); // A dropped discovery waiter cannot revive after settlement.
    assert_eq!(coordinator.pending_mcp_retirements(), 0);
    assert_eq!(
        coordinator
            .inner
            .source_preparation_slots
            .available_permits(),
        4
    );
    assert_eq!(control.established_generations(), 1);
    assert_eq!(control.catalog_requests(1), 0);
    assert_no_tool_effects(&control);
    assert_process_reaped(supervisor_pid);
    assert!(lifecycle.mark_quiescent());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn acquired_discovery_retains_unproven_stdio_settlement_before_releasing_its_slot() {
    if serve_if_recovery_fixture_mode().await {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let StdioPreparation { coordinator, lifecycle, id, generation, control, supervisor_pid } = stdio_preparation(
        &dir,
        "capabilities::coordinator::parallel_tests::acquired_catalog::acquired_discovery_retains_unproven_stdio_settlement_before_releasing_its_slot",
    ).await;
    let connection = generation.connection().clone();
    let pause = Arc::new(ConnectOwnershipPause::default());
    let _release = ReleasePause(pause.clone());
    connection.pause_acquired_discovery(pause.clone());
    let close = Arc::new(CloseProbe::failing_before_failure_publication(
        "acquired startup cleanup unproven",
    ));
    let _release_close = ReleaseClose(close.clone());
    connection
        .published_runtime()
        .unwrap()
        .install_close_probe(close.clone());
    let worker = rejected_preparation(coordinator.clone(), id, generation);
    tokio::time::timeout(WAIT, pause.wait_entered())
        .await
        .unwrap();
    connection.note_published_transport_loss_for_test("lost before startup listing");
    pause.release();
    tokio::time::timeout(WAIT, close.wait_before_failure_publication_entered())
        .await
        .unwrap();
    assert_eq!(coordinator.pending_mcp_retirements(), 1);
    assert_eq!(
        coordinator
            .inner
            .source_preparation_slots
            .available_permits(),
        3
    );
    assert!(!worker.is_finished());
    assert_process_reaped(supervisor_pid);
    close.release_before_failure_publication();
    let error = tokio::time::timeout(WAIT, worker)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("acquired startup cleanup unproven")
    );
    assert_eq!(
        coordinator
            .inner
            .source_preparation_slots
            .available_permits(),
        4
    );
    assert_eq!(connection.established_count().await, 1);
    assert_eq!(control.established_generations(), 1);
    assert_eq!(control.catalog_requests(1), 0);
    assert_no_tool_effects(&control);
    assert!(lifecycle.begin_drain());
    tokio::time::timeout(WAIT, lifecycle.wait_for_no_admissions())
        .await
        .unwrap();
    let failures = coordinator.drain_conversation_owned().await.unwrap_err();
    assert!(
        failures
            .iter()
            .any(|reason| reason.contains("acquired startup cleanup unproven"))
    );
    assert_eq!(
        coordinator.pending_mcp_retirements(),
        1,
        "unproven ownership evidence is never reaped"
    );
    assert!(
        coordinator
            .current_snapshot()
            .tool_registry()
            .definitions()
            .is_empty()
    );
}
