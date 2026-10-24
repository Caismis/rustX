//! Submission deadlines exercised through actual MCP negotiation and pagination.
use super::*;
use crate::capabilities::{CapabilitySourceState, selection::SourceToolSelection};
use crate::runtime::identity::McpServerId;
use crate::skills::{SkillDiscovery, SkillDiscoveryConfig};
use crate::tools::mcp::{McpServerBinding, McpTransportConfig};
use rmcp::model::{
    DiscoverResult, ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerInfo,
};
use rmcp::service::RequestContext;
use rmcp::{RoleServer, ServerHandler};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::sync::watch;

#[derive(Clone, Copy, PartialEq, Eq)]
enum HeldPhase {
    None,
    Negotiation,
    SecondPage,
}

#[derive(Clone)]
struct Peer {
    phase: HeldPhase,
    entered: watch::Sender<bool>,
    release: watch::Sender<bool>,
    discoveries: Arc<AtomicUsize>,
    lists: Arc<AtomicUsize>,
}

impl Peer {
    async fn hold(&self, context: RequestContext<RoleServer>) -> Result<(), rmcp::ErrorData> {
        self.entered.send_replace(true);
        let mut release = self.release.subscribe();
        tokio::select! {
            _ = release.wait_for(|released| *released) => Ok(()),
            () = context.ct.cancelled() => Err(rmcp::ErrorData::internal_error("cancelled", None)),
        }
    }
}

impl ServerHandler for Peer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
    }

    async fn discover(
        &self,
        context: RequestContext<RoleServer>,
    ) -> Result<DiscoverResult, rmcp::ErrorData> {
        self.discoveries.fetch_add(1, Ordering::SeqCst);
        if self.phase == HeldPhase::Negotiation {
            self.hold(context).await?;
        }
        Ok(DiscoverResult::from_server_info(
            crate::tools::mcp::supported_protocol_versions(),
            self.get_info(),
        ))
    }

    async fn list_tools(
        &self,
        request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, rmcp::ErrorData> {
        self.lists.fetch_add(1, Ordering::SeqCst);
        let second = request.and_then(|request| request.cursor).is_some();
        if self.phase == HeldPhase::SecondPage && second {
            self.hold(context).await?;
        }
        Ok(ListToolsResult {
            tools: vec![crate::tools::mcp::fixture::fixture_tool_named(if second {
                "second"
            } else {
                "first"
            })],
            next_cursor: (self.phase == HeldPhase::SecondPage && !second)
                .then(|| "second-page".to_owned()),
            ..Default::default()
        })
    }
}

struct HttpFixture {
    peer: Peer,
    binding: McpServerBinding,
    server: tokio::task::JoinHandle<()>,
    shutdown: tokio_util::sync::CancellationToken,
}

impl HttpFixture {
    async fn new(phase: HeldPhase) -> Self {
        use rmcp::transport::streamable_http_server::{
            StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
        };
        let peer = Peer {
            phase,
            entered: watch::channel(false).0,
            release: watch::channel(false).0,
            discoveries: Arc::default(),
            lists: Arc::default(),
        };
        let shutdown = tokio_util::sync::CancellationToken::new();
        let mut config = StreamableHttpServerConfig::default();
        config.cancellation_token = shutdown.child_token();
        config.sse_keep_alive = None;
        let handler = peer.clone();
        let service = StreamableHttpService::<Peer, LocalSessionManager>::new(
            move || Ok(handler.clone()),
            Arc::default(),
            config,
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, axum::Router::new().nest_service("/mcp", service))
                .await
                .unwrap();
        });
        Self {
            peer,
            binding: McpServerBinding {
                credentials: crate::credentials::SourceCredentials::default(),
                transport: McpTransportConfig::StreamableHttp {
                    endpoint: format!("http://{address}/mcp"),
                    headers: std::collections::BTreeMap::default(),
                },
                policy: crate::tools::types::ToolInvocationPolicy::default(),
            },
            server,
            shutdown,
        }
    }
}

impl Drop for HttpFixture {
    fn drop(&mut self) {
        self.shutdown.cancel();
        self.server.abort();
    }
}

async fn terminal_deadline(phase: HeldPhase) {
    let held = HttpFixture::new(phase).await;
    let healthy = HttpFixture::new(HeldPhase::None).await;
    let dir = tempfile::tempdir().unwrap();
    let (coordinator, held_id) = super::super::mcp_race_tests::coordinator_with_fixture(
        &dir,
        "held",
        "must-not-spawn-stdio",
    );
    let healthy_id = McpServerId::new("healthy");
    {
        let mut inputs = coordinator.inner.resource_inputs.lock().unwrap();
        inputs
            .mcp_servers
            .insert(held_id.clone(), held.binding.clone());
        inputs
            .mcp_servers
            .insert(healthy_id.clone(), healthy.binding.clone());
        inputs
            .source_demand
            .sources
            .insert(ToolSourceId::Mcp(healthy_id.clone()));
        inputs.agent_activation.profile.tools.sources.insert(
            ToolSourceId::Mcp(healthy_id.clone()),
            SourceToolSelection::All,
        );
    }
    let (completed, mut completions) = tokio::sync::mpsc::unbounded_channel();
    *coordinator.inner.mcp_completion.lock().unwrap() = Some(completed);
    let deadline = McpStartupDeadline::default();
    let worker_deadline = deadline.clone();
    let worker = coordinator.clone();
    let mut preparing = tokio::spawn(async move {
        let discovery =
            SkillDiscovery::with_config(&worker.inner.workspace, SkillDiscoveryConfig::default())
                .discover();
        worker
            .prepare_startup_candidate(
                discovery,
                BTreeSet::new(),
                worker_deadline,
                &CancellationSignal::new(),
            )
            .await
    });
    held.peer
        .entered
        .subscribe()
        .wait_for(|entered| *entered)
        .await
        .unwrap();
    assert_eq!(completions.recv().await.unwrap(), healthy_id);
    assert!(futures_util::FutureExt::now_or_never(&mut preparing).is_none());
    tokio::time::pause();
    let budget = if phase == HeldPhase::SecondPage {
        // Real pagination has already begun long before the first submission.
        tokio::time::advance(Duration::from_secs(60)).await;
        assert!(futures_util::FutureExt::now_or_never(&mut preparing).is_none());
        Duration::from_secs(15)
    } else {
        // Expire before the SDK's independent modern-to-legacy fallback.
        Duration::from_secs(3)
    };
    let first_deadline = tokio::time::Instant::now() + budget;
    assert!(deadline.arm(first_deadline));
    tokio::time::advance(budget.checked_sub(Duration::from_secs(1)).unwrap()).await;
    assert!(futures_util::FutureExt::now_or_never(&mut preparing).is_none());
    assert!(!deadline.arm(tokio::time::Instant::now() + budget));
    assert_eq!(
        coordinator
            .inner
            .source_preparation_slots
            .available_permits(),
        3
    );
    tokio::time::advance(Duration::from_secs(1)).await;
    assert_eq!(tokio::time::Instant::now(), first_deadline);
    tokio::time::resume();
    let candidate = tokio::time::timeout(Duration::from_secs(10), preparing)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_terminal_catalogs(&coordinator, candidate, held, healthy, held_id, healthy_id).await;
}

async fn assert_terminal_catalogs(
    coordinator: &CapabilityCoordinator,
    candidate: PreparedCapabilityCandidate,
    held: HttpFixture,
    healthy: HttpFixture,
    held_id: McpServerId,
    healthy_id: McpServerId,
) {
    assert!(matches!(
        candidate.availability().get(&ToolSourceId::Mcp(held_id)),
        Some(CapabilitySourceState::Unavailable { reason }) if reason.contains("first-submission wait deadline")
    ));
    assert_eq!(
        candidate.availability().get(&ToolSourceId::Mcp(healthy_id)),
        Some(&CapabilitySourceState::Ready)
    );
    assert_eq!(candidate.available_tools().tools().len(), 1);
    assert_eq!(held.peer.discoveries.load(Ordering::SeqCst), 1);
    assert_eq!(
        held.peer.lists.load(Ordering::SeqCst),
        if held.peer.phase == HeldPhase::SecondPage {
            2
        } else {
            0
        }
    );
    assert_eq!(healthy.peer.discoveries.load(Ordering::SeqCst), 1);
    assert_eq!(healthy.peer.lists.load(Ordering::SeqCst), 1);
    assert_eq!(coordinator.inner.mcp_preparations.load(Ordering::SeqCst), 2);
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
        1
    );
    // Releasing the obsolete response cannot resurrect its retired initializer.
    held.peer.release.send_replace(true);
    assert_eq!(candidate.mcp_runtimes.len(), 1);
    candidate.retire_uncommitted().await;
    assert_eq!(
        *coordinator
            .inner
            .mcp_invalidation
            .subscribe_catalog_changes()
            .borrow(),
        1
    );
}

#[tokio::test]
async fn submission_deadline_covers_complete_live_catalog_pagination() {
    terminal_deadline(HeldPhase::SecondPage).await;
}

#[tokio::test]
async fn submission_deadline_cancels_held_live_negotiation() {
    terminal_deadline(HeldPhase::Negotiation).await;
}

#[tokio::test]
async fn loading_owner_cancels_unarmed_paginated_preparation_and_awaits_settlement() {
    let held = HttpFixture::new(HeldPhase::SecondPage).await;
    let dir = tempfile::tempdir().unwrap();
    let (coordinator, id) = super::super::mcp_race_tests::coordinator_with_fixture(
        &dir,
        "held",
        "must-not-spawn-stdio",
    );
    coordinator
        .inner
        .resource_inputs
        .lock()
        .unwrap()
        .mcp_servers
        .insert(id, held.binding.clone());
    let cancellation = CancellationSignal::new();
    let worker_cancellation = cancellation.clone();
    let deadline = McpStartupDeadline::default();
    let worker_deadline = deadline.clone();
    let worker = coordinator.clone();
    let preparing = tokio::spawn(async move {
        let discovery =
            SkillDiscovery::with_config(&worker.inner.workspace, SkillDiscoveryConfig::default())
                .discover();
        worker
            .prepare_startup_candidate(
                discovery,
                BTreeSet::new(),
                worker_deadline,
                &worker_cancellation,
            )
            .await
    });
    held.peer
        .entered
        .subscribe()
        .wait_for(|entered| *entered)
        .await
        .unwrap();
    assert_eq!(deadline.deadline(), None);
    assert_eq!(
        coordinator
            .inner
            .source_preparation_slots
            .available_permits(),
        3
    );
    cancellation.cancel();
    let outcome = tokio::time::timeout(Duration::from_secs(10), preparing)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        outcome,
        Err(CapabilityPreparationError::ConversationInactive)
    ));
    assert_eq!(deadline.deadline(), None);
    assert_eq!(held.peer.discoveries.load(Ordering::SeqCst), 1);
    assert_eq!(held.peer.lists.load(Ordering::SeqCst), 2);
    assert_eq!(
        coordinator
            .inner
            .source_preparation_slots
            .available_permits(),
        4
    );
    assert_eq!(coordinator.pending_mcp_retirements(), 0);
    assert!(coordinator.current_snapshot().tool_registry().is_empty());
    assert_eq!(
        *coordinator
            .inner
            .mcp_invalidation
            .subscribe_catalog_changes()
            .borrow(),
        0
    );
}

#[tokio::test]
async fn completed_live_preparation_needs_no_wait_when_submission_arms_budget() {
    let fixture = HttpFixture::new(HeldPhase::None).await;
    let dir = tempfile::tempdir().unwrap();
    let (coordinator, id) = super::super::mcp_race_tests::coordinator_with_fixture(
        &dir,
        "ready",
        "must-not-spawn-stdio",
    );
    coordinator
        .inner
        .resource_inputs
        .lock()
        .unwrap()
        .mcp_servers
        .insert(id.clone(), fixture.binding.clone());
    let discovery = SkillDiscovery::with_config(
        &coordinator.inner.workspace,
        SkillDiscoveryConfig::default(),
    )
    .discover();
    let deadline = McpStartupDeadline::default();
    let ready = coordinator
        .prepare_startup_candidate(
            discovery,
            BTreeSet::new(),
            deadline.clone(),
            &CancellationSignal::new(),
        )
        .await
        .unwrap();
    assert_eq!(deadline.deadline(), None);
    assert_eq!(fixture.peer.discoveries.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.peer.lists.load(Ordering::SeqCst), 1);
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(60)).await;
    let received = tokio::time::Instant::now();
    assert!(deadline.arm(received + Duration::from_secs(15)));
    let published = coordinator.commit(ready).unwrap();
    assert_eq!(tokio::time::Instant::now(), received);
    assert_eq!(
        coordinator.availability().get(&ToolSourceId::Mcp(id)),
        Some(&CapabilitySourceState::Ready)
    );
    assert_eq!(published.available_tools().tools().len(), 1);
    assert_eq!(fixture.peer.discoveries.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.peer.lists.load(Ordering::SeqCst), 1);
    assert_eq!(coordinator.inner.mcp_preparations.load(Ordering::SeqCst), 1);
    tokio::time::resume();
    coordinator.retire_current_mcp_runtimes();
    coordinator.settle_ready_mcp_runtimes().await.unwrap();
    assert_eq!(coordinator.pending_mcp_retirements(), 0);
}

#[tokio::test]
async fn submission_deadline_bounds_eight_sources_including_unstarted_queue() {
    let mut fixtures = Vec::new();
    for index in 0..8 {
        fixtures.push((
            McpServerId::new(format!("batch-{index}")),
            HttpFixture::new(HeldPhase::Negotiation).await,
        ));
    }
    let dir = tempfile::tempdir().unwrap();
    let (coordinator, _) = super::super::mcp_race_tests::coordinator_with_fixture(
        &dir,
        "batch-0",
        "must-not-spawn-stdio",
    );
    {
        let mut inputs = coordinator.inner.resource_inputs.lock().unwrap();
        for (id, fixture) in &fixtures {
            inputs
                .mcp_servers
                .insert(id.clone(), fixture.binding.clone());
            let source = ToolSourceId::Mcp(id.clone());
            inputs.source_demand.sources.insert(source.clone());
            inputs
                .agent_activation
                .profile
                .tools
                .sources
                .insert(source, SourceToolSelection::All);
        }
    }
    let deadline = McpStartupDeadline::default();
    let worker_deadline = deadline.clone();
    let worker = coordinator.clone();
    let mut preparing = tokio::spawn(async move {
        let discovery =
            SkillDiscovery::with_config(&worker.inner.workspace, SkillDiscoveryConfig::default())
                .discover();
        worker
            .prepare_startup_candidate(
                discovery,
                BTreeSet::new(),
                worker_deadline,
                &CancellationSignal::new(),
            )
            .await
    });
    // Identity ordering and the four-slot limit put exactly the first four
    // sources in live negotiation; the other four have not acquired ownership.
    for (_, fixture) in fixtures.iter().take(4) {
        fixture
            .peer
            .entered
            .subscribe()
            .wait_for(|entered| *entered)
            .await
            .unwrap();
    }
    assert_eq!(coordinator.inner.mcp_preparations.load(Ordering::SeqCst), 4);
    assert_eq!(
        coordinator
            .inner
            .source_preparation_slots
            .available_permits(),
        0
    );
    assert!(futures_util::FutureExt::now_or_never(&mut preparing).is_none());
    tokio::time::pause();
    let submission_deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    assert!(deadline.arm(submission_deadline));
    tokio::time::advance(Duration::from_secs(2)).await;
    let later_owner = deadline.clone();
    assert!(!later_owner.arm(tokio::time::Instant::now() + Duration::from_secs(3)));
    assert!(futures_util::FutureExt::now_or_never(&mut preparing).is_none());
    tokio::time::advance(Duration::from_secs(1)).await;
    assert_eq!(tokio::time::Instant::now(), submission_deadline);
    tokio::time::resume();
    let candidate = tokio::time::timeout(Duration::from_secs(10), preparing)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_timed_out_batch(&coordinator, candidate, &fixtures);
    assert_eq!(deadline.deadline(), Some(submission_deadline));
    assert!(!later_owner.arm(tokio::time::Instant::now() + Duration::from_secs(3)));
}

fn assert_timed_out_batch(
    coordinator: &CapabilityCoordinator,
    candidate: PreparedCapabilityCandidate,
    fixtures: &[(McpServerId, HttpFixture)],
) {
    for (index, (id, fixture)) in fixtures.iter().enumerate() {
        assert!(matches!(
            candidate.availability().get(&ToolSourceId::Mcp(id.clone())),
            Some(CapabilitySourceState::Unavailable { reason })
                if reason.contains("first-submission wait deadline")
        ));
        assert_eq!(
            fixture.peer.discoveries.load(Ordering::SeqCst),
            usize::from(index < 4)
        );
        assert_eq!(fixture.peer.lists.load(Ordering::SeqCst), 0);
        // A response from the timed-out owner cannot turn it into Ready.
        fixture.peer.release.send_replace(true);
    }
    assert_eq!(coordinator.inner.mcp_preparations.load(Ordering::SeqCst), 4);
    assert_eq!(
        coordinator
            .inner
            .source_preparation_slots
            .available_permits(),
        4
    );
    assert_eq!(coordinator.pending_mcp_retirements(), 0);
    assert!(candidate.available_tools().tools().is_empty());
    assert!(candidate.mcp_runtimes.is_empty());
    coordinator.commit(candidate).unwrap();
    assert!(coordinator.current_snapshot().tool_registry().is_empty());
    assert_eq!(
        *coordinator
            .inner
            .mcp_invalidation
            .subscribe_catalog_changes()
            .borrow(),
        0
    );
}
