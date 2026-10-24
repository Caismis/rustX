//! One bounded live MCP preparation batch. A source reaches a terminal live
//! result before composition can publish any of its declarations.
#[cfg(all(test, feature = "mcp-fixture"))]
mod http_tests;

use super::{
    CapabilityCoordinator, CapabilityPreparationError, ConversationLifecycle,
    McpPreparationOutcome, PreparedCapabilityCandidate, ToolSourceId,
};
use crate::runtime::cancellation::CancellationSignal;
use std::collections::BTreeSet;
use std::future::Future;

/// The initial MCP batch has no elapsed-time deadline during preparation.
/// The native first-submission owner arms one shared absolute clock. An earlier
/// captured recovery receipt may tighten it; no waiter can extend it.
#[derive(Clone, Debug, Default)]
pub(crate) struct McpStartupDeadline {
    deadline: tokio::sync::watch::Sender<Option<tokio::time::Instant>>,
}

impl McpStartupDeadline {
    pub(crate) fn arm(&self, deadline: tokio::time::Instant) -> bool {
        self.deadline.send_if_modified(|current| {
            if current.is_some_and(|existing| existing <= deadline) {
                return false;
            }
            *current = Some(deadline);
            true
        })
    }

    pub(crate) fn deadline(&self) -> Option<tokio::time::Instant> {
        *self.deadline.borrow()
    }

    pub(crate) fn has_expired(&self) -> bool {
        self.deadline()
            .is_some_and(|deadline| tokio::time::Instant::now() >= deadline)
    }

    async fn expired(&self) {
        let mut changes = self.deadline.subscribe();
        loop {
            let deadline = *changes.borrow_and_update();
            if let Some(deadline) = deadline {
                tokio::select! {
                    biased;
                    () = tokio::time::sleep_until(deadline) => return,
                    notification = changes.changed() => {
                        notification.expect("the startup owner retains its deadline sender");
                    },
                }
            } else {
                changes
                    .changed()
                    .await
                    .expect("the startup owner retains its deadline sender");
            }
        }
    }
}

pub(super) struct InitialMcpPreparation {
    pub(super) required: BTreeSet<ToolSourceId>,
    pub(super) deadline: McpStartupDeadline,
}

pub(super) const MCP_PREPARATION_TIMEOUT: &str =
    "MCP live preparation exceeded the first-submission wait deadline";

/// Expiry cancels ownership, not the owning future. In-flight sources retain
/// their concurrency permits until physical settlement; queued work cannot start.
pub(super) async fn settle_by_deadline<F: Future>(
    preparation: F,
    deadline: &McpStartupDeadline,
    cancellation: &CancellationSignal,
) -> F::Output {
    tokio::pin!(preparation);
    tokio::select! {
        biased;
        () = deadline.expired() => {
            cancellation.cancel();
            preparation.await
        },
        outcome = &mut preparation => outcome,
    }
}

/// A source that answers after its terminal deadline cannot become a live
/// candidate. Successful siblings captured before the deadline remain usable.
pub(super) async fn finish_source_before_deadline(
    outcome: McpPreparationOutcome,
    deadline: &McpStartupDeadline,
) -> McpPreparationOutcome {
    if !deadline.has_expired() {
        return outcome;
    }
    let close = match outcome {
        Ok((_, generation, _)) => generation.retire_and_close().await,
        Err(reason) => return Err(format!("{MCP_PREPARATION_TIMEOUT}; {reason}")),
    };
    Err(close.map_or_else(
        || MCP_PREPARATION_TIMEOUT.to_owned(),
        |close| format!("{MCP_PREPARATION_TIMEOUT}; {close}"),
    ))
}

impl CapabilityCoordinator {
    pub(crate) fn preparation_lifecycle(&self) -> Option<ConversationLifecycle> {
        self.inner
            .preparation_lifecycle
            .lock()
            .expect("preparation lifecycle")
            .clone()
            .or_else(|| {
                self.inner
                    .state
                    .lock()
                    .expect("capability state")
                    .conversation_lifecycle
                    .clone()
            })
    }

    /// Initial composition shares this inactive lifecycle with the runtime that
    /// activates it. The finite selected closure completes live discovery first.
    pub(crate) async fn prepare_startup_candidate(
        &self,
        discovered: crate::skills::SkillDiscoveryOutcome,
        required: BTreeSet<ToolSourceId>,
        deadline: McpStartupDeadline,
        cancellation: &CancellationSignal,
    ) -> Result<PreparedCapabilityCandidate, CapabilityPreparationError> {
        self.inner
            .preparation_lifecycle
            .lock()
            .expect("preparation lifecycle")
            .get_or_insert_with(ConversationLifecycle::new);
        let (inputs, revision) = {
            let state = self.inner.state.lock().expect("capability state");
            (
                self.inner
                    .resource_inputs
                    .lock()
                    .expect("capability inputs")
                    .clone(),
                state.revision,
            )
        };
        self.prepare_owned(
            Some(cancellation),
            move |coordinator, cancellation| async move {
                coordinator
                    .prepare_candidate_inputs_owned(
                        inputs,
                        true,
                        revision,
                        Some(discovered),
                        Some(&cancellation),
                        Some(InitialMcpPreparation { required, deadline }),
                    )
                    .await
            },
        )
        .await
    }

    pub(crate) async fn settle_failed_startup(&self) -> Vec<String> {
        self.cancel_conversation_preparation();
        self.retire_current_mcp_runtimes();
        if let Some(lifecycle) = self.preparation_lifecycle() {
            lifecycle.wait_for_no_admissions().await;
        }
        self.inner.mcp_retirements.settle_all().await
    }

    /// Settle a rejected candidate's owners without cancelling the Conversation.
    pub(crate) async fn settle_rejected_startup(&self) -> Result<(), CapabilityPreparationError> {
        if let Some(lifecycle) = self.preparation_lifecycle() {
            lifecycle.wait_for_no_admissions().await;
        }
        let failures = self.inner.mcp_retirements.settle_all().await;
        if failures.is_empty() {
            Ok(())
        } else {
            Err(CapabilityPreparationError::Mcp(failures.join("; ")))
        }
    }
}

#[cfg(test)]
mod deadline_tests {
    use super::*;
    use futures_util::FutureExt;
    use std::sync::Arc;
    use std::time::Duration;

    #[tokio::test(start_paused = true)]
    async fn preparation_time_does_not_consume_first_submission_wait() {
        let slots = Arc::new(tokio::sync::Semaphore::new(1));
        let permit = slots.clone().acquire_owned().await.unwrap();
        let cancellation = CancellationSignal::new();
        let deadline = McpStartupDeadline::default();
        let (release, settled) = tokio::sync::oneshot::channel();
        let (entered, admitted) = tokio::sync::oneshot::channel();
        let owner = cancellation.clone();
        let preparation = async move {
            entered.send(()).unwrap();
            owner.cancelled().await;
            settled.await.unwrap();
            drop(permit);
            "physically settled"
        };
        let worker_signal = cancellation.clone();
        let worker_deadline = deadline.clone();
        let mut worker = tokio::spawn(async move {
            settle_by_deadline(preparation, &worker_deadline, &worker_signal).await
        });
        admitted.await.unwrap();
        tokio::time::advance(Duration::from_secs(60)).await;
        assert!(!cancellation.is_cancelled());
        assert!((&mut worker).now_or_never().is_none());
        assert!(deadline.arm(tokio::time::Instant::now() + Duration::from_secs(15)));
        tokio::time::advance(Duration::from_secs(14)).await;
        assert!(!cancellation.is_cancelled());
        tokio::time::advance(Duration::from_secs(1)).await;
        cancellation.cancelled().await;
        assert!((&mut worker).now_or_never().is_none());
        assert_eq!(slots.available_permits(), 0);
        release.send(()).unwrap();
        assert_eq!(worker.await.unwrap(), "physically settled");
        assert_eq!(slots.available_permits(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn earlier_captured_recovery_receipt_tightens_a_running_wait_without_extension() {
        let deadline = McpStartupDeadline::default();
        let receipt = tokio::time::Instant::now();
        assert!(deadline.arm(receipt + Duration::from_secs(25)));
        let (started, entered) = tokio::sync::oneshot::channel();
        let waiting = deadline.clone();
        let worker = tokio::spawn(async move {
            let expired = waiting.expired();
            tokio::pin!(expired);
            assert!(expired.as_mut().now_or_never().is_none());
            started.send(()).unwrap();
            expired.await;
            tokio::time::Instant::now()
        });
        entered.await.unwrap();
        tokio::time::advance(Duration::from_secs(10)).await;
        assert!(deadline.arm(receipt + Duration::from_secs(15)));
        assert!(!deadline.arm(receipt + Duration::from_secs(30)));
        tokio::time::advance(Duration::from_secs(4)).await;
        assert!(!worker.is_finished());
        tokio::time::advance(Duration::from_secs(1)).await;
        assert_eq!(worker.await.unwrap(), receipt + Duration::from_secs(15));
        assert!(deadline.has_expired());
        assert!(!deadline.arm(tokio::time::Instant::now() + Duration::from_secs(15)));
    }

    #[tokio::test(start_paused = true)]
    async fn queue_and_waiter_handoff_cannot_restart_the_first_submission_clock() {
        let cancellation = CancellationSignal::new();
        let deadline = McpStartupDeadline::default();
        let first = tokio::time::Instant::now() + Duration::from_secs(15);
        assert!(deadline.arm(first));
        let (release_queue, queued) = tokio::sync::oneshot::channel();
        let (phase_started, phase) = tokio::sync::oneshot::channel();
        let owner = cancellation.clone();
        let worker_signal = cancellation.clone();
        let worker_deadline = deadline.clone();
        let worker = tokio::spawn(async move {
            settle_by_deadline(
                async move {
                    queued.await.unwrap();
                    phase_started.send(()).unwrap();
                    owner.cancelled().await;
                },
                &worker_deadline,
                &worker_signal,
            )
            .await;
        });
        tokio::time::advance(Duration::from_secs(10)).await;
        release_queue.send(()).unwrap();
        phase.await.unwrap();
        assert!(!deadline.arm(tokio::time::Instant::now() + Duration::from_secs(15)));
        assert!(!cancellation.is_cancelled());
        tokio::time::advance(Duration::from_secs(5)).await;
        worker.await.unwrap();
        assert!(cancellation.is_cancelled());
        assert_eq!(tokio::time::Instant::now(), first);
    }
}

#[cfg(all(test, feature = "mcp-fixture"))]
mod tests {
    use super::*;
    use crate::capabilities::{CapabilityCommitError, CapabilitySourceState};
    use crate::skills::{SkillDiscovery, SkillDiscoveryConfig};
    use crate::tools::mcp::fixture::{FixtureServer, serve_if_fixture_mode};
    use crate::tools::mcp::test_sync::ConnectOwnershipPause;
    use std::sync::Arc;

    fn fixture(
        dir: &tempfile::TempDir,
        name: &str,
    ) -> (CapabilityCoordinator, crate::runtime::identity::McpServerId) {
        super::super::mcp_race_tests::coordinator_with_fixture(dir, "startup", name)
    }
    fn discovery(coordinator: &CapabilityCoordinator) -> crate::skills::SkillDiscoveryOutcome {
        SkillDiscovery::with_config(
            &coordinator.inner.workspace,
            SkillDiscoveryConfig::default(),
        )
        .discover()
    }

    #[tokio::test(start_paused = true)]
    async fn no_selected_mcp_needs_neither_a_budget_nor_a_preparation_slot() {
        let dir = tempfile::tempdir().unwrap();
        let (coordinator, id) = fixture(&dir, "unselected-must-never-spawn");
        {
            let mut inputs = coordinator.inner.resource_inputs.lock().unwrap();
            inputs.source_demand.sources.clear();
            inputs.agent_activation.profile.tools.sources.clear();
        }
        let held = coordinator
            .inner
            .source_preparation_slots
            .acquire_many(4)
            .await
            .unwrap();
        let deadline = McpStartupDeadline::default();
        let before = tokio::time::Instant::now();
        let candidate = coordinator
            .prepare_startup_candidate(
                discovery(&coordinator),
                BTreeSet::new(),
                deadline.clone(),
                &CancellationSignal::new(),
            )
            .await
            .unwrap();
        assert_eq!(tokio::time::Instant::now(), before);
        assert_eq!(deadline.deadline(), None);
        assert_eq!(
            candidate.availability().get(&ToolSourceId::Mcp(id)),
            Some(&CapabilitySourceState::Unprepared)
        );
        assert_eq!(
            coordinator
                .inner
                .mcp_preparations
                .load(std::sync::atomic::Ordering::Relaxed),
            0
        );
        assert!(candidate.available_tools().tools().is_empty());
        candidate.retire_uncommitted().await;
        drop(held);
        assert_eq!(
            coordinator
                .inner
                .source_preparation_slots
                .available_permits(),
            4
        );
    }

    #[tokio::test(start_paused = true)]
    async fn configured_source_queue_expires_without_starting_physical_work() {
        let dir = tempfile::tempdir().unwrap();
        let (coordinator, id) = fixture(&dir, "queue-must-never-spawn");
        let held = coordinator
            .inner
            .source_preparation_slots
            .acquire_many(4)
            .await
            .unwrap();
        let (waiting, mut queued) = tokio::sync::mpsc::unbounded_channel();
        *coordinator.inner.source_waiting.lock().unwrap() = Some(waiting);
        let deadline = McpStartupDeadline::default();
        let worker_deadline = deadline.clone();
        let worker = coordinator.clone();
        let mut preparing = tokio::spawn(async move {
            worker
                .prepare_startup_candidate(
                    discovery(&worker),
                    BTreeSet::new(),
                    worker_deadline,
                    &CancellationSignal::new(),
                )
                .await
        });
        queued.recv().await.unwrap();
        tokio::time::advance(std::time::Duration::from_secs(60)).await;
        assert!(futures_util::FutureExt::now_or_never(&mut preparing).is_none());
        deadline.arm(tokio::time::Instant::now() + std::time::Duration::from_secs(15));
        tokio::time::advance(std::time::Duration::from_secs(15)).await;
        let candidate = preparing.await.unwrap().unwrap();
        assert!(
            matches!(candidate.availability().get(&ToolSourceId::Mcp(id)), Some(CapabilitySourceState::Unavailable { reason }) if reason.contains("first-submission wait deadline"))
        );
        assert_eq!(
            coordinator
                .inner
                .mcp_preparations
                .load(std::sync::atomic::Ordering::Relaxed),
            0
        );
        assert_eq!(
            coordinator
                .inner
                .source_preparation_slots
                .available_permits(),
            0
        );
        assert!(candidate.available_tools().tools().is_empty());
        candidate.retire_uncommitted().await;
        drop(held);
        assert_eq!(
            coordinator
                .inner
                .source_preparation_slots
                .available_permits(),
            4
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn selected_root_waits_for_live_catalog_before_publication() {
        if serve_if_fixture_mode(FixtureServer::with_list_changed()).await {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let (coordinator, id) = fixture(
            &dir,
            "capabilities::coordinator::startup::tests::selected_root_waits_for_live_catalog_before_publication",
        );
        let pause = Arc::new(ConnectOwnershipPause::default());
        coordinator.install_connect_ownership_pause(pause.clone());
        let worker = coordinator.clone();
        let mut preparing = tokio::spawn(async move {
            worker
                .prepare_startup_candidate(
                    discovery(&worker),
                    BTreeSet::new(),
                    McpStartupDeadline::default(),
                    &CancellationSignal::new(),
                )
                .await
        });
        pause.wait_entered().await;
        assert!(futures_util::FutureExt::now_or_never(&mut preparing).is_none());
        assert!(coordinator.current_snapshot().tool_registry().is_empty());
        pause.release();
        let candidate = preparing.await.unwrap().unwrap();
        assert_eq!(
            candidate.availability().get(&ToolSourceId::Mcp(id)),
            Some(&CapabilitySourceState::Ready)
        );
        assert!(!candidate.available_tools().tools().is_empty());
        candidate.retire_uncommitted().await;
    }

    #[tokio::test]
    async fn expired_initialization_retires_a_late_process_before_returning_unavailable() {
        if serve_if_fixture_mode(FixtureServer::with_list_changed()).await {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let (coordinator, id) = fixture(
            &dir,
            "capabilities::coordinator::startup::tests::expired_initialization_retires_a_late_process_before_returning_unavailable",
        );
        let pause = Arc::new(ConnectOwnershipPause::default());
        coordinator.install_connect_ownership_pause(pause.clone());
        let deadline = McpStartupDeadline::default();
        let worker_deadline = deadline.clone();
        let worker = coordinator.clone();
        let mut preparing = tokio::spawn(async move {
            worker
                .prepare_startup_candidate(
                    discovery(&worker),
                    BTreeSet::new(),
                    worker_deadline,
                    &CancellationSignal::new(),
                )
                .await
        });
        pause.wait_entered().await;
        tokio::time::pause();
        tokio::time::advance(std::time::Duration::from_secs(60)).await;
        assert!(!deadline.has_expired());
        assert!(futures_util::FutureExt::now_or_never(&mut preparing).is_none());
        deadline.arm(tokio::time::Instant::now() + std::time::Duration::from_secs(15));
        tokio::time::advance(std::time::Duration::from_secs(15)).await;
        assert!(deadline.has_expired());
        assert!(futures_util::FutureExt::now_or_never(&mut preparing).is_none());
        assert_eq!(
            coordinator
                .inner
                .source_preparation_slots
                .available_permits(),
            3
        );
        tokio::time::resume();
        pause.release();
        let candidate = preparing.await.unwrap().unwrap();
        assert!(matches!(
            candidate.availability().get(&ToolSourceId::Mcp(id)),
            Some(CapabilitySourceState::Unavailable { .. })
        ));
        assert!(candidate.available_tools().tools().is_empty());
        assert_eq!(
            coordinator
                .inner
                .source_preparation_slots
                .available_permits(),
            4
        );
        assert_eq!(coordinator.pending_mcp_retirements(), 0);
        assert_eq!(
            *coordinator
                .inner
                .mcp_invalidation
                .subscribe_catalog_changes()
                .borrow(),
            0
        );
        candidate.retire_uncommitted().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn dropped_startup_waiter_keeps_process_counted_until_settled() {
        if serve_if_fixture_mode(FixtureServer::with_list_changed()).await {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let (coordinator, _) = fixture(
            &dir,
            "capabilities::coordinator::startup::tests::dropped_startup_waiter_keeps_process_counted_until_settled",
        );
        let pause = Arc::new(ConnectOwnershipPause::default());
        coordinator.install_connect_ownership_pause(pause.clone());
        let worker = coordinator.clone();
        let preparing = tokio::spawn(async move {
            worker
                .prepare_startup_candidate(
                    discovery(&worker),
                    BTreeSet::new(),
                    McpStartupDeadline::default(),
                    &CancellationSignal::new(),
                )
                .await
        });
        pause.wait_entered().await;
        preparing.abort();
        assert!(preparing.await.unwrap_err().is_cancelled());
        let lifecycle = coordinator.preparation_lifecycle().unwrap();
        assert!(lifecycle.activate());
        assert!(lifecycle.begin_drain());
        coordinator.cancel_conversation_preparation();
        assert!(!lifecycle.mark_quiescent());
        pause.release();
        coordinator.drain_conversation_owned().await.unwrap();
        lifecycle.wait_for_no_admissions().await;
        assert!(lifecycle.mark_quiescent());
        assert_eq!(coordinator.pending_mcp_retirements(), 0);
        assert!(coordinator.current_snapshot().tool_registry().is_empty());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn live_startup_catalog_cannot_commit_after_invalidation() {
        if serve_if_fixture_mode(FixtureServer::with_list_changed()).await {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let (coordinator, id) = fixture(
            &dir,
            "capabilities::coordinator::startup::tests::live_startup_catalog_cannot_commit_after_invalidation",
        );
        let candidate = coordinator
            .prepare_startup_candidate(
                discovery(&coordinator),
                [ToolSourceId::Mcp(id.clone())].into(),
                McpStartupDeadline::default(),
                &CancellationSignal::new(),
            )
            .await
            .unwrap();
        assert!(!candidate.available_tools().tools().is_empty());
        let captured_epoch = candidate.mcp_epochs[&id];
        coordinator.inner.mcp_invalidation.lock().advance(&id);
        assert_eq!(candidate.mcp_epochs[&id], captured_epoch);
        assert!(
            matches!(coordinator.commit(candidate), Err(CapabilityCommitError::StaleMcpCandidate { server_id }) if server_id == id)
        );
        coordinator.settle_rejected_startup().await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn failed_refresh_keeps_last_good_snapshot() {
        if serve_if_fixture_mode(FixtureServer::with_list_changed()).await {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let (coordinator, id) = fixture(
            &dir,
            "capabilities::coordinator::startup::tests::failed_refresh_keeps_last_good_snapshot",
        );
        let candidate = coordinator.prepare_candidate().await.unwrap();
        let original = coordinator.commit(candidate).unwrap();
        let mut inputs = coordinator.inner.resource_inputs.lock().unwrap().clone();
        let crate::tools::mcp::McpTransportConfig::Stdio { program, .. } =
            &mut inputs.mcp_servers.get_mut(&id).unwrap().transport
        else {
            unreachable!()
        };
        *program = dir.path().join("missing-program").display().to_string();
        assert!(matches!(
            coordinator.prepare_candidate_with_inputs(inputs).await,
            Err(CapabilityPreparationError::Mcp(_))
        ));
        assert!(Arc::ptr_eq(&original, &coordinator.current_snapshot()));
        coordinator.retire_current_mcp_runtimes();
        coordinator.settle_ready_mcp_runtimes().await.unwrap();
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn optional_failure_is_isolated_but_required_failure_rejects_the_closure() {
        if serve_if_fixture_mode(FixtureServer::with_list_changed()).await {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let (coordinator, healthy) = fixture(
            &dir,
            "capabilities::coordinator::startup::tests::optional_failure_is_isolated_but_required_failure_rejects_the_closure",
        );
        let failed = crate::runtime::identity::McpServerId::new("broken");
        let inert = crate::runtime::identity::McpServerId::new("unselected");
        {
            let mut inputs = coordinator.inner.resource_inputs.lock().unwrap();
            let mut binding = inputs.mcp_servers[&healthy].clone();
            let crate::tools::mcp::McpTransportConfig::Stdio { program, .. } =
                &mut binding.transport
            else {
                unreachable!()
            };
            *program = dir.path().join("missing-program").display().to_string();
            inputs.mcp_servers.insert(failed.clone(), binding.clone());
            inputs.mcp_servers.insert(inert.clone(), binding);
            inputs
                .source_demand
                .sources
                .insert(ToolSourceId::Mcp(failed.clone()));
            inputs.agent_activation.profile.tools.sources.insert(
                ToolSourceId::Mcp(failed.clone()),
                crate::capabilities::selection::SourceToolSelection::All,
            );
        }
        let candidate = coordinator
            .prepare_startup_candidate(
                discovery(&coordinator),
                BTreeSet::new(),
                McpStartupDeadline::default(),
                &CancellationSignal::new(),
            )
            .await
            .unwrap();
        assert_eq!(
            candidate.availability().get(&ToolSourceId::Mcp(healthy)),
            Some(&CapabilitySourceState::Ready)
        );
        assert!(matches!(
            candidate
                .availability()
                .get(&ToolSourceId::Mcp(failed.clone())),
            Some(CapabilitySourceState::Unavailable { .. })
        ));
        assert_eq!(
            candidate.availability().get(&ToolSourceId::Mcp(inert)),
            Some(&CapabilitySourceState::Unprepared)
        );
        assert!(!candidate.available_tools().tools().is_empty());
        assert_eq!(
            coordinator
                .inner
                .mcp_preparations
                .load(std::sync::atomic::Ordering::Relaxed),
            2
        );
        candidate.retire_uncommitted().await;
        assert!(matches!(
            coordinator
                .prepare_startup_candidate(
                    discovery(&coordinator),
                    [ToolSourceId::Mcp(failed)].into(),
                    McpStartupDeadline::default(),
                    &CancellationSignal::new(),
                )
                .await,
            Err(CapabilityPreparationError::Mcp(_))
        ));
        assert!(coordinator.current_snapshot().tool_registry().is_empty());
        assert_eq!(coordinator.pending_mcp_retirements(), 0);
    }

    #[tokio::test]
    async fn stale_initial_capture_retry_does_not_restart_the_first_submission_clock() {
        if serve_if_fixture_mode(FixtureServer::with_list_changed()).await {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let (coordinator, id) = fixture(
            &dir,
            "capabilities::coordinator::startup::tests::stale_initial_capture_retry_does_not_restart_the_first_submission_clock",
        );
        let deadline = McpStartupDeadline::default();
        let first = tokio::time::Instant::now() + std::time::Duration::from_secs(15);
        assert!(deadline.arm(first));
        let candidate = coordinator
            .prepare_startup_candidate(
                discovery(&coordinator),
                BTreeSet::new(),
                deadline.clone(),
                &CancellationSignal::new(),
            )
            .await
            .unwrap();
        coordinator.inner.mcp_invalidation.lock().advance(&id);
        assert!(matches!(
            coordinator.commit(candidate),
            Err(CapabilityCommitError::StaleMcpCandidate { .. })
        ));
        coordinator.settle_rejected_startup().await.unwrap();
        let starts = coordinator
            .inner
            .mcp_preparations
            .load(std::sync::atomic::Ordering::Relaxed);
        tokio::time::pause();
        tokio::time::advance(std::time::Duration::from_secs(15)).await;
        let retry = coordinator
            .prepare_startup_candidate(
                discovery(&coordinator),
                BTreeSet::new(),
                deadline.clone(),
                &CancellationSignal::new(),
            )
            .await
            .unwrap();
        assert_eq!(deadline.deadline(), Some(first));
        assert!(!deadline.arm(tokio::time::Instant::now() + std::time::Duration::from_secs(15)));
        assert_eq!(
            coordinator
                .inner
                .mcp_preparations
                .load(std::sync::atomic::Ordering::Relaxed),
            starts
        );
        assert!(matches!(
            retry.availability().get(&ToolSourceId::Mcp(id)),
            Some(CapabilitySourceState::Unavailable { .. })
        ));
        assert!(retry.available_tools().tools().is_empty());
        retry.retire_uncommitted().await;
    }
}
