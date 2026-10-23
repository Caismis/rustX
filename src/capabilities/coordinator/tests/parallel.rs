//! Source-preparation concurrency regressions use real MCP HTTP requests and
//! explicit server gates. Elapsed time never decides an interleaving.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use rmcp::model::{
    DiscoverResult, ListToolsResult, PaginatedRequestParams, ProtocolVersion, ServerCapabilities,
    ServerInfo,
};
use rmcp::service::RequestContext;
use rmcp::{RoleServer, ServerHandler};
use tokio::sync::watch;

use super::{CapabilityCoordinator, CapabilityCoordinatorConfig};
use crate::capabilities::selected::{SelectedCapabilityPlan, SelectedSourceTool};
use crate::capabilities::{CapabilityCommitError, CapabilitySourceState, ToolSourceId};
use crate::runtime::CancellationSignal;
use crate::runtime::identity::{ConversationId, McpServerId};
use crate::runtime::types::ConversationLifecycle;
use crate::tools::environment::ToolEnvironment;
use crate::tools::executor::ToolRegistry;
use crate::tools::mcp::{McpServerBinding, McpTransportConfig};
use crate::tools::workspace::Workspace;

const WAIT: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, Default)]
struct Handshakes {
    entered: usize,
    active: usize,
    peak: usize,
}

struct ActiveHandshake(watch::Sender<Handshakes>);

impl ActiveHandshake {
    fn enter(counts: &watch::Sender<Handshakes>) -> Self {
        counts.send_modify(|counts| {
            counts.entered += 1;
            counts.active += 1;
            counts.peak = counts.peak.max(counts.active);
        });
        Self(counts.clone())
    }
}

impl Drop for ActiveHandshake {
    fn drop(&mut self) {
        self.0.send_modify(|counts| counts.active -= 1);
    }
}

#[derive(Clone)]
struct Peer {
    tool_name: String,
    handshakes: watch::Sender<Handshakes>,
    handshake_calls: watch::Sender<usize>,
    handshake_release: watch::Sender<bool>,
    list_calls: watch::Sender<usize>,
    list_release: watch::Sender<bool>,
    cancelled_lists: watch::Sender<usize>,
    fail_listing: bool,
}

impl ServerHandler for Peer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
    }

    fn supported_protocol_versions(&self) -> std::borrow::Cow<'static, [ProtocolVersion]> {
        std::borrow::Cow::Owned(vec![ProtocolVersion::V_2026_07_28])
    }

    fn discover(
        &self,
        context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<DiscoverResult, rmcp::ErrorData>> + Send {
        let peer = self.clone();
        async move {
            let _active = ActiveHandshake::enter(&peer.handshakes);
            peer.handshake_calls.send_modify(|count| *count += 1);
            wait_release(&peer.handshake_release, &context).await?;
            Ok(DiscoverResult::from_server_info(
                vec![ProtocolVersion::V_2026_07_28],
                peer.get_info(),
            ))
        }
    }

    fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<ListToolsResult, rmcp::ErrorData>> + Send {
        let peer = self.clone();
        async move {
            peer.list_calls.send_modify(|count| *count += 1);
            if let Err(error) = wait_release(&peer.list_release, &context).await {
                peer.cancelled_lists.send_modify(|count| *count += 1);
                return Err(error);
            }
            if peer.fail_listing {
                return Err(rmcp::ErrorData::internal_error(
                    "this optional source cannot list its tools",
                    None,
                ));
            }
            Ok(ListToolsResult {
                tools: vec![crate::tools::mcp::fixture::fixture_tool_named(
                    &peer.tool_name,
                )],
                ..Default::default()
            })
        }
    }
}

async fn wait_release(
    release: &watch::Sender<bool>,
    context: &RequestContext<RoleServer>,
) -> Result<(), rmcp::ErrorData> {
    let mut released = release.subscribe();
    tokio::select! {
        result = released.wait_for(|released| *released) => {
            result.map(|_| ()).map_err(|_| rmcp::ErrorData::internal_error("fixture closed", None))
        }
        () = context.ct.cancelled() => {
            Err(rmcp::ErrorData::internal_error("the client terminated its request", None))
        }
    }
}

struct Fixture {
    id: McpServerId,
    peer: Peer,
    binding: McpServerBinding,
    cancellation: tokio_util::sync::CancellationToken,
    server: tokio::task::JoinHandle<()>,
}

impl Fixture {
    async fn start(id: &str, handshakes: &watch::Sender<Handshakes>, fail_listing: bool) -> Self {
        use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
        use rmcp::transport::streamable_http_server::{
            StreamableHttpServerConfig, StreamableHttpService,
        };

        let peer = Peer {
            tool_name: format!("{id}_echo"),
            handshakes: handshakes.clone(),
            handshake_calls: watch::channel(0).0,
            handshake_release: watch::channel(false).0,
            list_calls: watch::channel(0).0,
            list_release: watch::channel(false).0,
            cancelled_lists: watch::channel(0).0,
            fail_listing,
        };
        let cancellation = tokio_util::sync::CancellationToken::new();
        let mut config = StreamableHttpServerConfig::default();
        config.cancellation_token = cancellation.child_token();
        config.sse_keep_alive = None;
        let handler = peer.clone();
        let service = StreamableHttpService::<Peer, LocalSessionManager>::new(
            move || Ok(handler.clone()),
            Arc::default(),
            config,
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("fixture listener");
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, axum::Router::new().nest_service("/mcp", service))
                .await
                .unwrap();
        });
        Self {
            id: McpServerId::new(id),
            peer,
            binding: McpServerBinding {
                credentials: crate::credentials::SourceCredentials::default(),
                transport: McpTransportConfig::StreamableHttp {
                    endpoint: format!("http://{address}/mcp"),
                    headers: BTreeMap::new(),
                },
                policy: crate::tools::types::ToolInvocationPolicy::default(),
            },
            cancellation,
            server,
        }
    }

    fn release_handshake(&self) {
        self.peer.handshake_release.send_replace(true);
    }

    fn release_listing(&self) {
        self.peer.list_release.send_replace(true);
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.cancellation.cancel();
        self.server.abort();
    }
}

async fn wait_count(count: &watch::Sender<usize>, expected: usize) {
    tokio::time::timeout(WAIT, count.subscribe().wait_for(|count| *count >= expected))
        .await
        .expect("the expected request must reach its server")
        .expect("fixture counter");
}

fn coordinator(
    dir: &tempfile::TempDir,
    fixtures: &[Fixture],
    demanded: usize,
) -> (CapabilityCoordinator, ConversationLifecycle) {
    let workspace_root = dir.path().join("workspace");
    std::fs::create_dir(&workspace_root).unwrap();
    let workspace = Workspace::new(&workspace_root).unwrap();
    let sources: Vec<_> = fixtures
        .iter()
        .take(demanded)
        .map(|fixture| ToolSourceId::Mcp(fixture.id.clone()))
        .collect();
    let mut activation = crate::capabilities::AgentActivation::default();
    activation.profile.tools.sources = fixtures
        .iter()
        .take(demanded)
        .filter(|fixture| !fixture.peer.fail_listing)
        .map(|fixture| {
            (
                ToolSourceId::Mcp(fixture.id.clone()),
                crate::capabilities::selection::SourceToolSelection::All,
            )
        })
        .collect();
    let coordinator = CapabilityCoordinator::new(CapabilityCoordinatorConfig {
        source_demand: crate::capabilities::source::ToolSourceDemand::new(
            sources,
            crate::runtime::resources::ManagedPythonCatalog::default(),
        ),
        conversation_id: ConversationId::new("conv_14dafca7-8c38-7c20-8ac1-caa3300d9cab"),
        workspace: workspace.clone(),
        base_tool_registry: Arc::new(ToolRegistry::new()),
        extension_tools: crate::extensions::ExtensionToolPlane::none(),
        agent_activation: activation,
        skill_discovery: crate::skills::SkillDiscoveryConfig::workspace_root(
            workspace.root().join(".agents/skills"),
        ),
        mcp_servers: fixtures
            .iter()
            .map(|fixture| (fixture.id.clone(), fixture.binding.clone()))
            .collect(),
        base_environment: ToolEnvironment::new(),
        environment_store_root: dir.path().join("environments"),
    })
    .unwrap();
    let lifecycle = ConversationLifecycle::new();
    assert!(coordinator.claim_conversation_runtime(&lifecycle).is_some());
    assert!(lifecycle.activate());
    (coordinator, lifecycle)
}

fn selected_plan(fixtures: &[Fixture]) -> SelectedCapabilityPlan {
    SelectedCapabilityPlan {
        source_tools: fixtures
            .iter()
            .map(|fixture| {
                let tool = crate::tools::mcp::fixture::fixture_tool_named(&fixture.peer.tool_name);
                let policy = fixture.binding.policy;
                SelectedSourceTool {
                    source_id: ToolSourceId::Mcp(fixture.id.clone()),
                    name: fixture.peer.tool_name.clone(),
                    identity: crate::tools::mcp::identity::mcp_tool_identity(
                        &fixture.id,
                        &fixture.peer.tool_name,
                        tool.description.as_deref().unwrap_or_default(),
                        &serde_json::Value::Object((*tool.input_schema).clone()),
                        policy.execution,
                        policy.concurrency,
                        policy.approval,
                    ),
                }
            })
            .collect(),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mcp_handshakes_overlap_with_a_bound_of_four_and_undemanded_sources_stay_inert() {
    let dir = tempfile::tempdir().unwrap();
    let handshakes = watch::channel(Handshakes::default()).0;
    let mut fixtures = Vec::new();
    for id in ["alpha", "beta", "delta", "epsilon", "gamma", "undemanded"] {
        fixtures.push(Fixture::start(id, &handshakes, false).await);
    }
    for fixture in &fixtures {
        fixture.release_listing();
    }
    let (coordinator, _) = coordinator(&dir, &fixtures, 5);
    let preparing = coordinator.clone();
    let worker = tokio::spawn(async move { preparing.prepare_candidate().await });
    tokio::time::timeout(
        WAIT,
        handshakes
            .subscribe()
            .wait_for(|counts| counts.entered >= 4),
    )
    .await
    .expect("four handshakes must overlap")
    .unwrap();
    assert_eq!(handshakes.borrow().active, 4);
    assert_eq!(
        coordinator
            .inner
            .source_preparation_slots
            .available_permits(),
        0
    );
    assert_eq!(*fixtures[4].peer.handshake_calls.borrow(), 0);
    assert_eq!(*fixtures[5].peer.handshake_calls.borrow(), 0);
    fixtures[0].release_handshake();
    wait_count(&fixtures[4].peer.handshake_calls, 1).await;
    for fixture in fixtures.iter().take(5).rev() {
        fixture.release_handshake();
    }
    let candidate = tokio::time::timeout(WAIT, worker)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(handshakes.borrow().peak, 4);
    assert_eq!(candidate.mcp_runtimes.len(), 5);
    assert_eq!(*fixtures[5].peer.handshake_calls.borrow(), 0);
    assert_eq!(
        candidate.availability()[&ToolSourceId::Mcp(fixtures[5].id.clone())],
        CapabilitySourceState::Unprepared
    );
    candidate.retire_uncommitted().await;
    coordinator.drain_conversation_owned().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn simultaneous_candidates_share_the_coordinators_source_preparation_bound() {
    let dir = tempfile::tempdir().unwrap();
    let handshakes = watch::channel(Handshakes::default()).0;
    let mut fixtures = Vec::new();
    for id in ["alpha", "beta", "gamma"] {
        fixtures.push(Fixture::start(id, &handshakes, false).await);
    }
    for fixture in &fixtures {
        fixture.release_listing();
    }
    let (coordinator, _) = coordinator(&dir, &fixtures, fixtures.len());
    let preparing = coordinator.clone();
    let first = tokio::spawn(async move { preparing.prepare_candidate().await });
    for fixture in &fixtures {
        wait_count(&fixture.peer.handshake_calls, 1).await;
    }
    let preparing = coordinator.clone();
    let second = tokio::spawn(async move { preparing.prepare_candidate().await });
    tokio::time::timeout(
        WAIT,
        handshakes
            .subscribe()
            .wait_for(|counts| counts.entered >= 4),
    )
    .await
    .expect("the second candidate must share the remaining concurrency slot")
    .unwrap();
    assert_eq!(handshakes.borrow().active, 4);
    assert_eq!(
        coordinator
            .inner
            .source_preparation_slots
            .available_permits(),
        0
    );
    for fixture in &fixtures {
        fixture.release_handshake();
    }
    for worker in [first, second] {
        tokio::time::timeout(WAIT, worker)
            .await
            .unwrap()
            .unwrap()
            .unwrap()
            .retire_uncommitted()
            .await;
    }
    assert_eq!(handshakes.borrow().entered, 6);
    assert_eq!(handshakes.borrow().peak, 4);
    coordinator.drain_conversation_owned().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(clippy::too_many_lines)] // One scenario compares both completion orders and every publication output.
async fn reverse_mcp_completion_keeps_identity_order_and_isolates_optional_failure() {
    let dir = tempfile::tempdir().unwrap();
    let handshakes = watch::channel(Handshakes::default()).0;
    let mut fixtures = Vec::new();
    for (id, fails) in [
        ("alpha", false),
        ("beta", true),
        ("gamma", false),
        ("undemanded", false),
    ] {
        fixtures.push(Fixture::start(id, &handshakes, fails).await);
    }
    for fixture in &fixtures {
        fixture.release_handshake();
    }
    let (coordinator, _) = coordinator(&dir, &fixtures, 3);
    let (completed_tx, mut completed_rx) = tokio::sync::mpsc::unbounded_channel();
    *coordinator.inner.mcp_completion.lock().unwrap() = Some(completed_tx);
    let native_read = crate::tools::native::definitions(
        crate::tools::native::NativeToolPolicies::default(),
        &crate::runtime::subagent::AgentCatalog::default(),
    )
    .into_iter()
    .map(|(definition, _)| definition)
    .find(|definition| definition.name == "read")
    .expect("native Read definition");
    let mut base = ToolRegistry::new();
    base.register(
        native_read.clone(),
        Arc::new(crate::tools::native::ReadTool),
    )
    .unwrap();
    {
        let mut inputs = coordinator.inner.resource_inputs.lock().unwrap();
        inputs.base_tool_registry = Arc::new(base);
        inputs.agent_activation.profile.tools.builtin = vec!["read".into()];
    }
    let preparing = coordinator.clone();
    let worker = tokio::spawn(async move { preparing.prepare_candidate().await });
    for fixture in fixtures.iter().take(3) {
        wait_count(&fixture.peer.list_calls, 1).await;
    }
    for fixture in fixtures.iter().take(3).rev() {
        fixture.release_listing();
        assert_eq!(
            tokio::time::timeout(WAIT, completed_rx.recv())
                .await
                .unwrap(),
            Some(fixture.id.clone()),
            "each client preparation completes before the next source is released"
        );
    }
    let candidate = tokio::time::timeout(WAIT, worker)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let ids: Vec<_> = candidate
        .mcp_runtimes
        .iter()
        .map(|generation| generation.server_id().as_str())
        .collect();
    assert_eq!(ids, ["alpha", "gamma"]);
    let tool_ids: Vec<_> = candidate
        .available_tools()
        .tools()
        .iter()
        .map(|tool| tool.definition.id.as_str())
        .collect();
    let expected_ids = [
        native_read.id.as_str().to_owned(),
        crate::tools::mcp::mcp_tool_id(&fixtures[0].id, &fixtures[0].peer.tool_name),
        crate::tools::mcp::mcp_tool_id(&fixtures[2].id, &fixtures[2].peer.tool_name),
    ];
    assert_eq!(tool_ids, expected_ids);
    assert!(
        candidate
            .candidate_registry
            .definitions()
            .contains(&native_read)
    );
    assert!(matches!(
        candidate.availability()[&ToolSourceId::Mcp(fixtures[1].id.clone())],
        CapabilitySourceState::Unavailable { .. }
    ));
    assert_eq!(
        candidate.availability()[&ToolSourceId::Mcp(fixtures[0].id.clone())],
        CapabilitySourceState::Ready
    );
    assert_eq!(
        candidate.availability()[&ToolSourceId::Mcp(fixtures[2].id.clone())],
        CapabilitySourceState::Ready
    );
    let reverse_tools = candidate.available_tools().definitions();
    let reverse_registry = candidate.candidate_registry.definitions();
    let reverse_availability = candidate.availability().clone();
    let reverse_profile = candidate.resolved_profile().cloned();
    let reverse_ids: Vec<_> = candidate
        .mcp_runtimes
        .iter()
        .map(|generation| generation.server_id().clone())
        .collect();
    candidate.retire_uncommitted().await;
    for fixture in fixtures.iter().take(3) {
        fixture.peer.list_release.send_replace(false);
    }
    let preparing = coordinator.clone();
    let worker = tokio::spawn(async move { preparing.prepare_candidate().await });
    for fixture in fixtures.iter().take(3) {
        wait_count(&fixture.peer.list_calls, 2).await;
    }
    for fixture in fixtures.iter().take(3) {
        fixture.release_listing();
        assert_eq!(
            tokio::time::timeout(WAIT, completed_rx.recv())
                .await
                .unwrap(),
            Some(fixture.id.clone())
        );
    }
    let candidate = tokio::time::timeout(WAIT, worker)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(candidate.available_tools().definitions(), reverse_tools);
    assert_eq!(candidate.candidate_registry.definitions(), reverse_registry);
    assert_eq!(candidate.availability(), &reverse_availability);
    assert_eq!(candidate.resolved_profile(), reverse_profile.as_ref());
    assert_eq!(
        candidate
            .mcp_runtimes
            .iter()
            .map(|generation| generation.server_id().clone())
            .collect::<Vec<_>>(),
        reverse_ids
    );
    assert_eq!(*fixtures[3].peer.handshake_calls.borrow(), 0);
    candidate.retire_uncommitted().await;
    coordinator.drain_conversation_owned().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn selected_required_failure_settles_every_parallel_source_before_returning() {
    let dir = tempfile::tempdir().unwrap();
    let handshakes = watch::channel(Handshakes::default()).0;
    let mut fixtures = Vec::new();
    for id in ["alpha", "beta", "gamma", "unselected"] {
        fixtures.push(Fixture::start(id, &handshakes, false).await);
    }
    for fixture in &fixtures {
        fixture.release_handshake();
    }
    let (coordinator, lifecycle) = coordinator(&dir, &fixtures, fixtures.len());
    let mut plan = selected_plan(&fixtures);
    plan.source_tools.truncate(3);
    plan.source_tools[0].name = "removed".into();
    let preparing = coordinator.clone();
    let worker = tokio::spawn(async move {
        preparing
            .prepare_selected_candidate(&plan, &CancellationSignal::new())
            .await
    });
    for fixture in fixtures.iter().take(3) {
        wait_count(&fixture.peer.list_calls, 1).await;
    }
    fixtures[0].release_listing();
    assert!(
        !worker.is_finished(),
        "a required failure still owes its blocked sibling owners"
    );
    fixtures[2].release_listing();
    fixtures[1].release_listing();
    let error = tokio::time::timeout(WAIT, worker)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert!(error.to_string().contains("removed"), "{error}");
    assert_eq!(*fixtures[3].peer.handshake_calls.borrow(), 0);
    assert_eq!(coordinator.pending_mcp_retirements(), 0);
    tokio::time::timeout(WAIT, lifecycle.wait_for_no_admissions())
        .await
        .expect("the result sender releases its final preparation admission");
    assert!(lifecycle.begin_drain());
    assert!(
        lifecycle.mark_quiescent(),
        "every candidate owner must settle before the error is returned"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dropped_parallel_preparation_cancels_lists_and_settles_owners() {
    let dir = tempfile::tempdir().unwrap();
    let handshakes = watch::channel(Handshakes::default()).0;
    let mut fixtures = Vec::new();
    for id in ["alpha", "beta", "gamma", "omega", "queued"] {
        fixtures.push(Fixture::start(id, &handshakes, false).await);
    }
    for fixture in &fixtures {
        fixture.release_handshake();
    }
    let (coordinator, lifecycle) = coordinator(&dir, &fixtures, fixtures.len());
    let preparing = coordinator.clone();
    let worker = tokio::spawn(async move { preparing.prepare_candidate().await });
    for fixture in fixtures.iter().take(4) {
        wait_count(&fixture.peer.list_calls, 1).await;
    }
    worker.abort();
    assert!(worker.await.unwrap_err().is_cancelled());
    assert!(lifecycle.begin_drain());
    tokio::time::timeout(WAIT, lifecycle.wait_for_no_admissions())
        .await
        .expect("every cancelled candidate owner must settle");
    tokio::time::timeout(WAIT, coordinator.drain_conversation_owned())
        .await
        .expect("drain must cancel every outstanding catalog request")
        .unwrap();
    for fixture in fixtures.iter().take(4) {
        wait_count(&fixture.peer.cancelled_lists, 1).await;
    }
    assert_eq!(*fixtures[4].peer.handshake_calls.borrow(), 0);
    assert_eq!(coordinator.pending_mcp_retirements(), 0);
    assert!(lifecycle.mark_quiescent());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn invalidated_parallel_candidate_preserves_the_last_good_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let handshakes = watch::channel(Handshakes::default()).0;
    let mut fixtures = Vec::new();
    for id in ["alpha", "beta"] {
        fixtures.push(Fixture::start(id, &handshakes, false).await);
    }
    for fixture in &fixtures {
        fixture.release_handshake();
        fixture.release_listing();
    }
    let (coordinator, _) = coordinator(&dir, &fixtures, fixtures.len());
    coordinator.release_conversation_runtime_claim();
    let initial = coordinator.prepare_candidate().await.unwrap();
    let snapshot = coordinator.commit(initial).unwrap();
    let availability = coordinator.availability();
    for fixture in &fixtures {
        fixture.peer.list_release.send_replace(false);
    }
    let preparing = coordinator.clone();
    let worker = tokio::spawn(async move { preparing.prepare_candidate().await });
    for fixture in &fixtures {
        wait_count(&fixture.peer.list_calls, 2).await;
    }
    for fixture in fixtures.iter().rev() {
        fixture.release_listing();
    }
    let candidate = tokio::time::timeout(WAIT, worker)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    coordinator
        .inner
        .mcp_invalidation
        .lock()
        .advance(&fixtures[0].id);
    assert!(matches!(
        coordinator.commit(candidate),
        Err(CapabilityCommitError::StaleMcpCandidate { .. })
    ));
    assert!(Arc::ptr_eq(&coordinator.current_snapshot(), &snapshot));
    assert_eq!(coordinator.availability(), availability);
    coordinator.settle_ready_mcp_runtimes().await.unwrap();
    coordinator.drain_conversation_owned().await.unwrap();
    assert_eq!(coordinator.pending_mcp_retirements(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancelled_catalog_reconnect_retains_ownership_until_physical_settlement() {
    use crate::tools::mcp::fixture::{
        FIXTURE_MODE_ENV, FixtureServer, fixture_spawn_args, serve_if_fixture_mode,
    };
    use crate::tools::mcp::test_sync::ConnectOwnershipPause;

    if serve_if_fixture_mode(FixtureServer::with_list_changed()).await {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let (coordinator, lifecycle) = coordinator(&dir, &[], 0);
    let server_id = McpServerId::new("replacement");
    let binding = McpServerBinding {
        credentials: crate::credentials::SourceCredentials::default(),
        transport: McpTransportConfig::Stdio {
            program: std::env::current_exe().unwrap().display().to_string(),
            args: fixture_spawn_args(
                "capabilities::coordinator::parallel_tests::cancelled_catalog_reconnect_retains_ownership_until_physical_settlement",
            ),
            cwd: None,
            environment: [(FIXTURE_MODE_ENV.to_owned(), "1".to_owned())].into(),
        },
        policy: crate::tools::types::ToolInvocationPolicy::default(),
    };
    let generation = coordinator
        .connect_conversation_owned(&server_id, &binding, None)
        .await
        .unwrap();
    generation
        .connection()
        .published_runtime()
        .unwrap()
        .close()
        .await
        .unwrap();
    let pause = Arc::new(ConnectOwnershipPause::default());
    generation
        .connection()
        .pause_reconnect_ownership(pause.clone());
    let preparing = coordinator.clone();
    let caller = tokio::spawn(async move {
        preparing
            .prepare_owned(None, move |coordinator, cancellation| async move {
                let mut outcomes = coordinator
                    .prepare_sources([generation], |generation| {
                        coordinator.prepare_mcp_generation(
                            &server_id,
                            generation,
                            Some(&cancellation),
                        )
                    })
                    .await;
                match outcomes.pop().unwrap() {
                    Ok((_, generation, _)) => {
                        generation.retire_and_close().await;
                        panic!("the cancelled replacement must not become a candidate")
                    }
                    Err(reason) => {
                        Err(crate::capabilities::CapabilityPreparationError::Mcp(reason))
                    }
                }
            })
            .await
    });
    tokio::time::timeout(WAIT, pause.wait_entered())
        .await
        .expect("the replacement owns a physical stdio process before cancellation");
    assert!(pause.supervisor_pid().is_some());
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    assert!(lifecycle.begin_drain());
    tokio::time::timeout(WAIT, async {
        while coordinator.pending_mcp_retirements() == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("cancellation retires the generation before awaiting its replacement owner");
    assert!(!lifecycle.mark_quiescent());
    assert_eq!(
        coordinator
            .inner
            .source_preparation_slots
            .available_permits(),
        3
    );
    pause.release();
    tokio::time::timeout(WAIT, lifecycle.wait_for_no_admissions())
        .await
        .expect("the cancelled replacement settles before releasing its owner admission");
    coordinator.drain_conversation_owned().await.unwrap();
    assert_eq!(coordinator.pending_mcp_retirements(), 0);
    assert_eq!(
        coordinator
            .inner
            .source_preparation_slots
            .available_permits(),
        4
    );
    assert!(lifecycle.mark_quiescent());
}
