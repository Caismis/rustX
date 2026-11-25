//! Submission deadlines exercised through actual MCP negotiation and pagination.
use super::*;
use crate::capabilities::{CapabilitySourceState, ToolSourceId, selection::SourceToolSelection};
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
    FirstPage,
    RepeatedCursor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ObservedCredential {
    Old,
    Rotated,
}

#[derive(Clone)]
struct Peer {
    phase: HeldPhase,
    entered: watch::Sender<bool>,
    release: watch::Sender<bool>,
    discoveries: Arc<AtomicUsize>,
    lists: Arc<AtomicUsize>,
    calls: Arc<std::sync::Mutex<Vec<ObservedCredential>>>,
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
        if (self.phase == HeldPhase::SecondPage && second)
            || (self.phase == HeldPhase::FirstPage && !second)
        {
            self.hold(context).await?;
        }
        Ok(ListToolsResult {
            tools: vec![crate::tools::mcp::fixture::fixture_tool_named(if second {
                "second"
            } else {
                "first"
            })],
            next_cursor: ((self.phase == HeldPhase::SecondPage && !second)
                || self.phase == HeldPhase::RepeatedCursor)
                .then(|| "second-page".to_owned()),
            ..Default::default()
        })
    }
    fn call_tool(
        &self,
        request: rmcp::model::CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<rmcp::model::CallToolResponse, rmcp::ErrorData>>
    {
        std::future::ready((|| {
            if request.name != "first" {
                return Err(rmcp::ErrorData::invalid_params(
                    "unexpected fixture tool",
                    None,
                ));
            }
            let header = context
                .extensions
                .get::<http::request::Parts>()
                .and_then(|parts| parts.headers.get(http::header::AUTHORIZATION))
                .and_then(|header| header.to_str().ok());
            let credential = match header {
                Some("Bearer rotation-old-fixture") => ObservedCredential::Old,
                Some("Bearer rotation-new-fixture") => ObservedCredential::Rotated,
                _ => {
                    return Err(rmcp::ErrorData::invalid_params(
                        "missing fixture credential",
                        None,
                    ));
                }
            };
            self.calls.lock().unwrap().push(credential);
            Ok(rmcp::model::CallToolResult::success(vec![]).into())
        })())
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
            calls: Arc::default(),
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
                permissions: None,
                tool_permissions: std::collections::BTreeMap::new(),
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
        let discovery = SkillDiscovery::with_config(SkillDiscoveryConfig::default()).discover();
        worker
            .prepare_startup_candidate(discovery, worker_deadline, &CancellationSignal::new())
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
        let discovery = SkillDiscovery::with_config(SkillDiscoveryConfig::default()).discover();
        worker
            .prepare_startup_candidate(discovery, worker_deadline, &worker_cancellation)
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
    let discovery = SkillDiscovery::with_config(SkillDiscoveryConfig::default()).discover();
    let deadline = McpStartupDeadline::default();
    let ready = coordinator
        .prepare_startup_candidate(discovery, deadline.clone(), &CancellationSignal::new())
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
        let discovery = SkillDiscovery::with_config(SkillDiscoveryConfig::default()).discover();
        worker
            .prepare_startup_candidate(discovery, worker_deadline, &CancellationSignal::new())
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
    assert_eq!(candidate.available_tools().tools(), []);
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

#[tokio::test]
async fn same_authority_can_publish_declarations_without_waiting_for_live_http() {
    let fixture = HttpFixture::new(HeldPhase::FirstPage).await;
    fixture.peer.release.send_replace(true);
    let dir = tempfile::tempdir().unwrap();
    let make_coordinator = || {
        let (coordinator, id) = super::super::mcp_race_tests::coordinator_with_fixture(
            &dir,
            "cache",
            "must-not-spawn-stdio",
        );
        coordinator
            .inner
            .resource_inputs
            .lock()
            .unwrap()
            .mcp_servers
            .insert(id.clone(), fixture.binding.clone());
        (coordinator, id)
    };
    let prepare = |coordinator: CapabilityCoordinator| async move {
        let skills = SkillDiscovery::with_config(SkillDiscoveryConfig::default()).discover();
        coordinator
            .prepare_startup_candidate(
                skills,
                McpStartupDeadline::default(),
                &CancellationSignal::new(),
            )
            .await
            .unwrap()
    };
    let (initial, _) = make_coordinator();
    let initial_candidate = prepare(initial.clone()).await;
    assert_eq!(initial_candidate.available_tools().tools().len(), 1);
    initial_candidate.retire_uncommitted().await;
    assert_eq!(fixture.peer.discoveries.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.peer.lists.load(Ordering::SeqCst), 1);

    fixture.peer.release.send_replace(false);
    fixture.peer.entered.send_replace(false);
    let (next, id) = make_coordinator();
    let cached = prepare(next.clone()).await;
    assert_eq!(cached.available_tools().tools().len(), 1);
    assert_eq!(fixture.peer.discoveries.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.peer.lists.load(Ordering::SeqCst), 1);
    let connection = cached.mcp_runtimes[0].connection().clone();
    assert!(connection.catalog_snapshot().is_none());

    let fresh = tokio::spawn(async move { connection.discover_catalog().await.unwrap() });
    fixture
        .peer
        .entered
        .subscribe()
        .wait_for(|entered| *entered)
        .await
        .unwrap();
    assert!(!fresh.is_finished());
    assert_eq!(fixture.peer.discoveries.load(Ordering::SeqCst), 2);
    assert_eq!(fixture.peer.lists.load(Ordering::SeqCst), 2);
    fixture.peer.release.send_replace(true);
    assert_eq!(fresh.await.unwrap()[0].name, "first");
    cached.retire_uncommitted().await;

    let (workflow, _) = make_coordinator();
    workflow
        .inner
        .resource_inputs
        .lock()
        .unwrap()
        .source_demand
        .live_mcp
        .insert(ToolSourceId::Mcp(id));
    let current = prepare(workflow).await;
    assert_eq!(current.available_tools().tools().len(), 1);
    assert_eq!(fixture.peer.discoveries.load(Ordering::SeqCst), 3);
    assert_eq!(fixture.peer.lists.load(Ordering::SeqCst), 3);
    assert!(
        current.mcp_runtimes[0]
            .connection()
            .catalog_snapshot()
            .is_some()
    );
    current.retire_uncommitted().await;
}

#[tokio::test]
async fn repeated_http_catalog_cursor_refuses_declarations_without_more_requests() {
    let fixture = HttpFixture::new(HeldPhase::RepeatedCursor).await;
    let dir = tempfile::tempdir().unwrap();
    let (coordinator, id) = super::super::mcp_race_tests::coordinator_with_fixture(
        &dir,
        "repeated-cursor",
        "must-not-spawn-stdio",
    );
    coordinator
        .inner
        .resource_inputs
        .lock()
        .unwrap()
        .mcp_servers
        .insert(id.clone(), fixture.binding.clone());
    let skills = SkillDiscovery::with_config(SkillDiscoveryConfig::default()).discover();
    let candidate = coordinator
        .prepare_startup_candidate(
            skills,
            McpStartupDeadline::default(),
            &CancellationSignal::new(),
        )
        .await
        .unwrap();
    assert!(
        matches!(candidate.availability().get(&ToolSourceId::Mcp(id)),
        Some(CapabilitySourceState::Unavailable { reason }) if reason.contains("repeated a cursor"))
    );
    assert_eq!(candidate.available_tools().tools(), []);
    assert_eq!(fixture.peer.discoveries.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.peer.lists.load(Ordering::SeqCst), 2);
    candidate.retire_uncommitted().await;
    assert_eq!(coordinator.pending_mcp_retirements(), 0);
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // Scoped fixture compares two physical admissions and their close.
async fn rotation_with_identical_http_declarations_keeps_old_lease_and_fences_new_admissions() {
    use crate::tools::executor::{ProgressReporter, ToolExecutionContext, ToolRegistration};
    use crate::tools::types::{
        ToolExecutionStatus, ToolInvocation, ToolInvocationId, ToolInvocationMode,
    };
    struct Silent;
    impl ProgressReporter for Silent {
        fn report(&self, _: crate::tools::types::ToolProgress) {}
    }
    let fixture = HttpFixture::new(HeldPhase::None).await;
    let dir = tempfile::tempdir().unwrap();
    let (coordinator, id) = super::super::mcp_race_tests::coordinator_with_fixture(
        &dir,
        "rotation",
        "must-not-spawn-stdio",
    );
    let mut inputs = coordinator.inner.resource_inputs.lock().unwrap().clone();
    let binding = |secret: Option<&str>| {
        let mut binding = fixture.binding.clone();
        binding.credentials = serde_json::from_value(serde_json::json!({
            "headers": {"authorization": "$ROTATION_TOKEN"}
        }))
        .unwrap();
        binding
            .credentials
            .capture(crate::credentials::CredentialSnapshot::new(
                secret.map(|secret| ("ROTATION_TOKEN".into(), secret.into())),
            ));
        binding
    };
    inputs
        .mcp_servers
        .insert(id.clone(), binding(Some("Bearer rotation-old-fixture")));
    let first = coordinator
        .prepare_candidate_with_inputs(inputs.clone())
        .await
        .unwrap();
    let old = coordinator.commit(first).unwrap();
    let old_leases = old.acquire_mcp_leases().unwrap();

    inputs.mcp_servers.insert(id.clone(), binding(None));
    assert!(
        coordinator
            .prepare_candidate_with_inputs(inputs.clone())
            .await
            .is_err()
    );
    assert_eq!(coordinator.current_snapshot().revision(), old.revision());

    inputs
        .mcp_servers
        .insert(id, binding(Some("Bearer rotation-new-fixture")));
    let candidate = coordinator
        .prepare_candidate_with_inputs(inputs)
        .await
        .unwrap();
    assert_eq!(
        candidate.candidate_registry.definitions(),
        old.tool_registry().definitions()
    );
    let new = coordinator.commit(candidate).unwrap();
    assert_eq!(new.revision().get(), old.revision().get() + 1);
    assert_eq!(coordinator.pending_mcp_retirements(), 1);

    let tools = crate::tools::runtime::ConversationToolRuntime::new(
        coordinator.inner.conversation_id.clone(),
        coordinator.inner.workspace.root(),
        dir.path().join("artifacts"),
    )
    .unwrap();
    let run = |entry: ToolRegistration, call_id: &'static str| {
        let tools = &tools;
        async move {
            entry
                .executor
                .start(
                    ToolInvocation {
                        id: ToolInvocationId::Agent {
                            call_id: crate::runtime::identity::ToolCallId::new(call_id),
                        },
                        tool_id: entry.definition.id,
                        tool_name: entry.definition.name,
                        mode: ToolInvocationMode::Foreground,
                        arguments: serde_json::json!({}),
                    },
                    ToolExecutionContext::new(
                        tools.conversation_id(),
                        None,
                        crate::runtime::ExecutionCancellation::detached(
                            CancellationSignal::new(),
                            crate::runtime::types::CancellationReason::UserRequested,
                        ),
                        tools.workspace(),
                        &Silent,
                        tools.artifacts(),
                        tools.tool_output(),
                        tools.environment(),
                    ),
                )
                .completion
                .await
        }
    };
    let current = new.available_tools().registrations()[0].clone();
    let frozen = old.available_tools().registrations()[0].clone();
    assert_eq!(
        run(current, "new").await.status,
        ToolExecutionStatus::Success
    );
    assert_eq!(
        run(frozen.clone(), "old").await.status,
        ToolExecutionStatus::Success
    );
    assert_eq!(
        *fixture.peer.calls.lock().unwrap(),
        [ObservedCredential::Rotated, ObservedCredential::Old]
    );
    assert_eq!(fixture.peer.discoveries.load(Ordering::SeqCst), 2);
    assert_eq!(fixture.peer.lists.load(Ordering::SeqCst), 2);

    drop(old_leases);
    coordinator.settle_ready_mcp_runtimes().await.unwrap();
    assert_eq!(coordinator.pending_mcp_retirements(), 0);
    assert!(matches!(
        run(frozen, "retired").await.status,
        ToolExecutionStatus::Failed { .. }
    ));
    assert_eq!(fixture.peer.calls.lock().unwrap().len(), 2);
    coordinator.drain_conversation_owned().await.unwrap();
}
