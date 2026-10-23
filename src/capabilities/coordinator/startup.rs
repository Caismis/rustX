//! One finite optional startup batch. Definitions remain candidate-owned; this
//! owner only completes live discovery and signals the native adoption owner.
use super::{
    CapabilityCoordinator, CapabilityPreparationError, ConversationLifecycle,
    McpPreparationOutcome, McpRuntimeGeneration, McpServerId, PreparedCapabilityCandidate,
    ToolSourceId,
};
use crate::tools::mcp::catalog_cache::{CatalogCacheKey, CatalogHint, McpCatalogCache};
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

const OPTIONAL_STARTUP_GRACE: std::time::Duration = std::time::Duration::from_secs(1);

// One deadline for the entire optional batch. Hits need no transport wait.
async fn wait_optional_grace(
    changed: &mut tokio::sync::watch::Receiver<StartupProgress>,
    uncached: &BTreeSet<McpServerId>,
) {
    if uncached.is_empty() {
        return;
    }
    let _ = tokio::time::timeout(OPTIONAL_STARTUP_GRACE, async {
        loop {
            if uncached.is_subset(&changed.borrow().completed) {
                break;
            }
            if changed.changed().await.is_err() {
                break;
            }
        }
    })
    .await;
}

pub(super) struct ConfiguredPreparation {
    pub(super) outcomes: Vec<(McpServerId, McpPreparationOutcome)>,
    pub(super) hints: BTreeMap<McpServerId, CatalogHint>,
    pub(super) pending: BTreeSet<McpServerId>,
}

#[derive(Default)]
pub(super) struct StartupReadiness {
    completed: tokio::sync::watch::Sender<StartupProgress>,
    pending: AtomicBool,
}

#[derive(Clone, Default)]
pub(crate) struct StartupProgress {
    pub(crate) completed: BTreeSet<McpServerId>,
    pub(crate) done: bool,
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

    /// Initial composition shares this exact inactive lifecycle with the runtime
    /// that will activate it. Optional work is counted before starting any I/O.
    pub(crate) async fn prepare_startup_candidate(
        &self,
        discovered: crate::skills::SkillDiscoveryOutcome,
        required: BTreeSet<ToolSourceId>,
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
        self.prepare_owned(None, move |coordinator, cancellation| async move {
            coordinator
                .prepare_candidate_inputs_owned(
                    inputs,
                    true,
                    revision,
                    Some(discovered),
                    Some(&cancellation),
                    Some(required),
                )
                .await
        })
        .await
    }

    /// Composition failure cancels and settles the same counted owners that a
    /// constructed runtime would drain; no initializer escapes a failed root.
    pub(crate) async fn settle_failed_startup(&self) -> Vec<String> {
        self.cancel_conversation_preparation();
        self.retire_current_mcp_runtimes();
        if let Some(lifecycle) = self.preparation_lifecycle() {
            lifecycle.wait_for_no_admissions().await;
        }
        self.inner.mcp_retirements.settle_all().await
    }

    /// A stale initial candidate has already dropped its unique generations.
    /// Settle those owners before the one bounded startup retry; the shared
    /// cancellation root stays open for the replacement preparation.
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

    /// Readiness is an inert notification. Only normal configuration preparation
    /// and adoption can publish the completed directory.
    pub(crate) fn startup_readiness(
        &self,
    ) -> Option<tokio::sync::watch::Receiver<StartupProgress>> {
        self.inner
            .startup
            .pending
            .load(Ordering::Acquire)
            .then(|| self.inner.startup.completed.subscribe())
    }

    pub(super) async fn prepare_configured_sources(
        &self,
        bindings: Vec<(McpServerId, crate::tools::mcp::McpServerBinding)>,
        cancellation: Option<&crate::runtime::cancellation::CancellationSignal>,
        required: Option<BTreeSet<ToolSourceId>>,
    ) -> ConfiguredPreparation {
        let Some(required) = required else {
            return ConfiguredPreparation {
                outcomes: self.prepare_mcp_bindings(bindings, cancellation).await,
                hints: BTreeMap::new(),
                pending: BTreeSet::new(),
            };
        };
        let (required, optional): (Vec<_>, Vec<_>) = bindings
            .into_iter()
            .partition(|(id, _)| required.contains(&ToolSourceId::Mcp(id.clone())));
        let (mut required, mut optional) = tokio::join!(
            self.prepare_mcp_bindings(required, cancellation),
            self.prepare_optional_startup(optional),
        );
        self.finalize_optional_startup(&mut optional).await;
        required.append(&mut optional.outcomes);
        optional.outcomes = required;
        optional
    }

    /// Required Workflow discovery can outlast optional grace. Capture any
    /// optional completion that won before the final candidate is assembled.
    async fn finalize_optional_startup(&self, result: &mut ConfiguredPreparation) {
        for (id, outcome) in std::mem::take(&mut result.outcomes) {
            let outcome = match outcome {
                Ok((mut epoch, generation, mut tools)) if result.pending.contains(&id) => {
                    match generation.connection().catalog_snapshot() {
                        Some((captured_epoch, Ok(live))) => {
                            tools = live;
                            epoch = captured_epoch;
                            result.hints.remove(&id);
                            result.pending.remove(&id);
                            Ok((epoch, generation, tools))
                        }
                        Some((_, Err(error))) => {
                            result.hints.remove(&id);
                            result.pending.remove(&id);
                            let close = generation.retire_and_close().await;
                            Err(close.map_or(error.clone(), |close| format!("{error}; {close}")))
                        }
                        None => {
                            if result.hints.get(&id).is_some_and(|hint| !hint.valid()) {
                                result.hints.remove(&id);
                                tools.clear();
                                epoch = self.inner.mcp_invalidation.epoch(&id);
                            }
                            Ok((epoch, generation, tools))
                        }
                    }
                }
                outcome => outcome,
            };
            result.outcomes.push((id, outcome));
        }
        self.inner
            .startup
            .pending
            .store(!result.pending.is_empty(), Ordering::Release);
    }

    fn start_optional_initializers(
        &self,
        connections: Vec<(
            McpServerId,
            Arc<crate::tools::mcp::McpConnection>,
            crate::tools::mcp::McpCatalogStartup,
        )>,
        lifecycle: &ConversationLifecycle,
    ) -> tokio::sync::watch::Receiver<StartupProgress> {
        let admission = lifecycle
            .try_enter_preparation()
            .expect("startup composition admission")
            .into_resident_resource();
        let coordinator = self.clone();
        let completed = self.inner.startup.completed.clone();
        completed.send_replace(StartupProgress::default());
        let changed = completed.subscribe();
        tokio::spawn(async move {
            coordinator
                .prepare_sources(
                    connections,
                    &coordinator.inner.preparation_cancellation,
                    |(id, connection, startup)| {
                        let completed = &completed;
                        #[cfg(test)]
                        let coordinator = &coordinator;
                        async move {
                            #[cfg(test)]
                            coordinator
                                .inner
                                .mcp_preparations
                                .fetch_add(1, Ordering::Relaxed);
                            let _ = connection.discover_catalog().await;
                            drop(startup);
                            #[cfg(test)]
                            if let Some(completion) =
                                &*coordinator.inner.mcp_completion.lock().unwrap()
                            {
                                let _ = completion.send(id.clone());
                            }
                            completed.send_modify(|sources| {
                                sources.completed.insert(id);
                            });
                        }
                    },
                )
                .await;
            // Dropping the counted batch guard is later than every initializer.
            coordinator
                .inner
                .startup
                .completed
                .send_modify(|progress| progress.done = true);
            drop(admission);
        });
        changed
    }

    async fn prepare_optional_startup(
        &self,
        bindings: Vec<(McpServerId, crate::tools::mcp::McpServerBinding)>,
    ) -> ConfiguredPreparation {
        let mut result = ConfiguredPreparation {
            outcomes: Vec::new(),
            hints: BTreeMap::new(),
            pending: BTreeSet::new(),
        };
        if bindings.is_empty() {
            return result;
        }
        let lifecycle = self.preparation_lifecycle().expect("startup lifecycle");
        let cache = McpCatalogCache::process();
        let mut generations = BTreeMap::new();
        let mut connections = Vec::new();
        let mut uncached = BTreeSet::new();
        for (id, binding) in bindings {
            let key = CatalogCacheKey::for_binding(&id, &binding, &self.inner.workspace);
            self.inner
                .mcp_invalidation
                .attach_cache_key(&id, key.clone());
            if let Some(hint) = key.as_ref().and_then(|key| cache.lookup(key)) {
                result.hints.insert(id.clone(), hint);
            } else {
                uncached.insert(id.clone());
            }
            let Ok(admission) = lifecycle.try_enter_preparation() else {
                result
                    .outcomes
                    .push((id, Err("MCP startup cancelled".into())));
                continue;
            };
            let connection = crate::tools::mcp::McpConnection::lazy(
                id.clone(),
                binding,
                self.inner.workspace.clone(),
                self.inner.mcp_invalidation.clone(),
                self.inner.preparation_cancellation.child(),
            );
            #[cfg(all(test, feature = "mcp-fixture"))]
            if let Some(pause) = self
                .inner
                .connect_ownership_pause
                .lock()
                .expect("connect pause")
                .clone()
            {
                connection.pause_reconnect_ownership(pause);
            }
            let generation = McpRuntimeGeneration::from_connection(
                id.clone(),
                connection.clone(),
                Some(admission),
                tokio::runtime::Handle::current(),
                &self.inner.mcp_retirements,
            );
            let startup = connection.begin_startup();
            connections.push((id.clone(), connection, startup));
            generations.insert(id, generation);
        }
        let mut changed = self.start_optional_initializers(connections, &lifecycle);
        wait_optional_grace(&mut changed, &uncached).await;
        for (id, generation) in generations {
            let epoch = self.inner.mcp_invalidation.epoch(&id);
            match generation.connection().catalog_snapshot() {
                Some((epoch, Ok(tools))) => {
                    result.hints.remove(&id);
                    result.outcomes.push((id, Ok((epoch, generation, tools))));
                }
                Some((_, Err(error))) => {
                    result.hints.remove(&id);
                    let close = generation.retire_and_close().await;
                    result.outcomes.push((
                        id,
                        Err(close.map_or(error.clone(), |close| format!("{error}; {close}"))),
                    ));
                }
                None => {
                    let tools =
                        if let Some(hint) = result.hints.get(&id).filter(|hint| hint.valid()) {
                            hint.tools().to_vec()
                        } else {
                            result.hints.remove(&id);
                            Vec::new()
                        };
                    result.pending.insert(id.clone());
                    result.outcomes.push((id, Ok((epoch, generation, tools))));
                }
            }
        }
        self.inner
            .startup
            .pending
            .store(!result.pending.is_empty(), Ordering::Release);
        result
    }
}

#[cfg(all(test, feature = "mcp-fixture"))]
mod tests {
    use super::*;
    use crate::capabilities::{CapabilityCommitError, CapabilitySourceState};
    use crate::skills::{SkillDiscovery, SkillDiscoveryConfig};
    use crate::tools::mcp::fixture::{FixtureServer, serve_if_fixture_mode};
    use crate::tools::mcp::test_sync::ConnectOwnershipPause;

    fn fixture(dir: &tempfile::TempDir, name: &str) -> (CapabilityCoordinator, McpServerId) {
        super::super::mcp_race_tests::coordinator_with_fixture(dir, "startup", name)
    }
    fn discovery(coordinator: &CapabilityCoordinator) -> crate::skills::SkillDiscoveryOutcome {
        SkillDiscovery::with_config(
            &coordinator.inner.workspace,
            SkillDiscoveryConfig::default(),
        )
        .discover()
    }
    async fn ready(coordinator: &CapabilityCoordinator) {
        let Some(mut readiness) = coordinator.startup_readiness() else {
            return;
        };
        tokio::time::timeout(std::time::Duration::from_secs(15), async {
            while !readiness.borrow_and_update().done {
                readiness.changed().await.unwrap();
            }
        })
        .await
        .expect("startup batch completes");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn cold_optional_grace_keeps_the_admitted_snapshot_immutable() {
        if serve_if_fixture_mode(FixtureServer::with_list_changed()).await {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let (coordinator, id) = fixture(
            &dir,
            "capabilities::coordinator::startup::tests::cold_optional_grace_keeps_the_admitted_snapshot_immutable",
        );
        let pause = Arc::new(ConnectOwnershipPause::default());
        coordinator.install_connect_ownership_pause(pause.clone());
        let worker = coordinator.clone();
        let preparing = tokio::spawn(async move {
            worker
                .prepare_startup_candidate(discovery(&worker), BTreeSet::new())
                .await
        });
        pause.wait_entered().await;
        let candidate = tokio::time::timeout(std::time::Duration::from_secs(10), preparing)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(
            candidate.availability().get(&ToolSourceId::Mcp(id.clone())),
            Some(&CapabilitySourceState::Connecting { cached: false })
        );
        assert!(candidate.available_tools().tools().is_empty());
        let snapshot = coordinator.commit(candidate).unwrap();
        let attempt = coordinator.acquire_attempt_lease();
        pause.release();
        ready(&coordinator).await;
        assert!(snapshot.tool_registry().is_empty());
        assert!(attempt.snapshot().tool_registry().is_empty());
        assert_eq!(
            coordinator.availability().get(&ToolSourceId::Mcp(id)),
            Some(&CapabilitySourceState::Connecting { cached: false })
        );
        let refreshed = coordinator.prepare_candidate().await.unwrap();
        assert!(!refreshed.available_tools().tools().is_empty());
        assert!(matches!(
            coordinator.commit(refreshed),
            Err(CapabilityCommitError::Busy)
        ));
        drop(attempt);
        let refreshed = coordinator.prepare_candidate().await.unwrap();
        let published = coordinator.commit(refreshed).unwrap();
        assert!(!published.tool_registry().is_empty());
        assert!(snapshot.tool_registry().is_empty());
        coordinator.retire_current_mcp_runtimes();
        coordinator.settle_ready_mcp_runtimes().await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn cached_optional_catalog_is_frozen_before_transport_readiness() {
        if serve_if_fixture_mode(FixtureServer::with_list_changed()).await {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let name = "capabilities::coordinator::startup::tests::cached_optional_catalog_is_frozen_before_transport_readiness";
        let (warm, _) = fixture(&dir, name);
        let first = warm.prepare_candidate().await.unwrap();
        let expected = first.available_tools().tools().len();
        assert!(expected > 0);
        first.retire_uncommitted().await;
        let (coordinator, id) = fixture(&dir, name);
        let pause = Arc::new(ConnectOwnershipPause::default());
        coordinator.install_connect_ownership_pause(pause.clone());
        let candidate = coordinator
            .prepare_startup_candidate(discovery(&coordinator), BTreeSet::new())
            .await
            .unwrap();
        assert_eq!(candidate.available_tools().tools().len(), expected);
        assert_eq!(
            candidate.availability().get(&ToolSourceId::Mcp(id)),
            Some(&CapabilitySourceState::Connecting { cached: true })
        );
        assert_eq!(candidate.mcp_hints.len(), 1);
        pause.wait_entered().await;
        let snapshot = coordinator.commit(candidate).unwrap();
        pause.release();
        ready(&coordinator).await;
        assert_eq!(snapshot.available_tools().tools().len(), expected);
        coordinator.retire_current_mcp_runtimes();
        coordinator.settle_ready_mcp_runtimes().await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn required_workflow_source_cannot_fall_through_optional_grace() {
        if serve_if_fixture_mode(FixtureServer::with_list_changed()).await {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let (coordinator, id) = fixture(
            &dir,
            "capabilities::coordinator::startup::tests::required_workflow_source_cannot_fall_through_optional_grace",
        );
        let pause = Arc::new(ConnectOwnershipPause::default());
        coordinator.install_connect_ownership_pause(pause.clone());
        let worker = coordinator.clone();
        let required = [ToolSourceId::Mcp(id.clone())].into();
        let mut preparing = tokio::spawn(async move {
            worker
                .prepare_startup_candidate(discovery(&worker), required)
                .await
        });
        pause.wait_entered().await;
        assert!(futures_util::FutureExt::now_or_never(&mut preparing).is_none());
        pause.release();
        let candidate = preparing.await.unwrap().unwrap();
        assert_eq!(
            candidate.availability().get(&ToolSourceId::Mcp(id)),
            Some(&CapabilitySourceState::Ready)
        );
        assert!(!candidate.available_tools().tools().is_empty());
        assert!(candidate.mcp_hints.is_empty());
        assert!(coordinator.startup_readiness().is_none());
        candidate.retire_uncommitted().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn pending_initializer_stays_counted_until_drain_settles_the_process() {
        if serve_if_fixture_mode(FixtureServer::with_list_changed()).await {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let (coordinator, _) = fixture(
            &dir,
            "capabilities::coordinator::startup::tests::pending_initializer_stays_counted_until_drain_settles_the_process",
        );
        let pause = Arc::new(ConnectOwnershipPause::default());
        coordinator.install_connect_ownership_pause(pause.clone());
        let worker = coordinator.clone();
        let preparing = tokio::spawn(async move {
            worker
                .prepare_startup_candidate(discovery(&worker), BTreeSet::new())
                .await
        });
        pause.wait_entered().await;
        let candidate = preparing.await.unwrap().unwrap();
        coordinator.commit(candidate).unwrap();
        let lifecycle = coordinator.preparation_lifecycle().unwrap();
        assert!(coordinator.claim_conversation_runtime(&lifecycle).is_some());
        assert!(lifecycle.activate());
        assert!(lifecycle.begin_drain());
        coordinator.cancel_conversation_preparation();
        coordinator.retire_current_mcp_runtimes();
        assert!(!lifecycle.mark_quiescent());
        pause.release();
        tokio::time::timeout(
            std::time::Duration::from_secs(15),
            coordinator.drain_conversation_owned(),
        )
        .await
        .unwrap()
        .unwrap();
        lifecycle.wait_for_no_admissions().await;
        assert!(lifecycle.mark_quiescent());
        assert_eq!(coordinator.pending_mcp_retirements(), 0);
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn another_conversation_invalidates_the_hint_before_commit_and_retry_settles() {
        if serve_if_fixture_mode(FixtureServer::with_list_changed()).await {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let name = "capabilities::coordinator::startup::tests::another_conversation_invalidates_the_hint_before_commit_and_retry_settles";
        let (warm, _) = fixture(&dir, name);
        warm.prepare_candidate()
            .await
            .unwrap()
            .retire_uncommitted()
            .await;
        let (coordinator, id) = fixture(&dir, name);
        let pause = Arc::new(ConnectOwnershipPause::default());
        coordinator.install_connect_ownership_pause(pause.clone());
        let candidate = coordinator
            .prepare_startup_candidate(discovery(&coordinator), BTreeSet::new())
            .await
            .unwrap();
        assert_eq!(candidate.mcp_hints.len(), 1);
        pause.wait_entered().await;
        let key = CatalogCacheKey::for_binding(
            &id,
            &candidate.effective_mcp_servers[&id],
            &coordinator.inner.workspace,
        )
        .unwrap();
        let other = crate::tools::mcp::McpInvalidationState::new();
        other.attach_cache_key(&id, Some(key));
        other.lock().advance(&id);
        assert!(matches!(
            coordinator.commit(candidate),
            Err(CapabilityCommitError::StaleMcpCandidate { .. })
        ));
        pause.release();
        coordinator.settle_rejected_startup().await.unwrap();
        let retry = coordinator
            .prepare_startup_candidate(discovery(&coordinator), BTreeSet::new())
            .await
            .unwrap();
        assert!(retry.mcp_hints.is_empty());
        let published = coordinator.commit(retry).unwrap();
        assert!(!published.tool_registry().is_empty());
        coordinator.retire_current_mcp_runtimes();
        coordinator.settle_ready_mcp_runtimes().await.unwrap();
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
        // Required live discovery eliminates grace/cache timing from this
        // capture/publication ordering: the candidate owns real definitions.
        let candidate = coordinator
            .prepare_startup_candidate(
                discovery(&coordinator),
                [ToolSourceId::Mcp(id.clone())].into(),
            )
            .await
            .unwrap();
        assert!(candidate.mcp_hints.is_empty());
        assert!(!candidate.available_tools().tools().is_empty());
        assert_eq!(
            candidate.availability().get(&ToolSourceId::Mcp(id.clone())),
            Some(&CapabilitySourceState::Ready)
        );
        let captured_epoch = candidate.mcp_epochs[&id];
        coordinator.inner.mcp_invalidation.lock().advance(&id);
        assert_eq!(
            coordinator.inner.mcp_invalidation.epoch(&id),
            captured_epoch + 1
        );
        assert_eq!(
            candidate.mcp_epochs[&id], captured_epoch,
            "captured live tools must never be stamped with a later epoch"
        );
        let before = coordinator.current_snapshot().revision();
        assert!(
            matches!(coordinator.commit(candidate), Err(CapabilityCommitError::StaleMcpCandidate { server_id }) if server_id == id)
        );
        assert_eq!(coordinator.current_snapshot().revision(), before);
        coordinator.settle_rejected_startup().await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn failed_refresh_keeps_last_good_snapshot_and_revokes_changed_source_hint() {
        if serve_if_fixture_mode(FixtureServer::with_list_changed()).await {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let (coordinator, id) = fixture(
            &dir,
            "capabilities::coordinator::startup::tests::failed_refresh_keeps_last_good_snapshot_and_revokes_changed_source_hint",
        );
        let candidate = coordinator.prepare_candidate().await.unwrap();
        let key = CatalogCacheKey::for_binding(
            &id,
            &candidate.effective_mcp_servers[&id],
            &coordinator.inner.workspace,
        )
        .unwrap();
        let hint = McpCatalogCache::process()
            .lookup(&key)
            .expect("live capture");
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
        assert!(!hint.valid());
        assert!(Arc::ptr_eq(&original, &coordinator.current_snapshot()));
        assert!(!original.tool_registry().is_empty());
        coordinator.retire_current_mcp_runtimes();
        coordinator.settle_ready_mcp_runtimes().await.unwrap();
    }
}
