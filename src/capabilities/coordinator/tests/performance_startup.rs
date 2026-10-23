//! Controlled HTTP startup measurements. Composition is a model-admission
//! boundary proxy, not an observed provider request; first tool dispatch is
//! measured by the real server handler. Correctness tests use barriers instead.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, DiscoverResult, ListToolsResult,
    PaginatedRequestParams, ServerCapabilities, ServerInfo,
};
use rmcp::service::RequestContext;
use rmcp::{RoleServer, ServerHandler};
use serde::Serialize;

use super::super::{CapabilityCoordinator, CapabilityCoordinatorConfig};
use crate::capabilities::{AgentActivation, CapabilitySourceState, ToolSourceId};
use crate::runtime::identity::{ConversationId, McpServerId, ToolCallId};
use crate::tools::executor::{ProgressReporter, ToolExecutionContext, ToolRegistry};
use crate::tools::mcp::catalog_cache::{CatalogCacheKey, McpCatalogCache};
use crate::tools::mcp::{McpServerBinding, McpServerBindings, McpTransportConfig};
use crate::tools::runtime::ConversationToolRuntime;
use crate::tools::types::{ToolInvocation, ToolInvocationId, ToolInvocationMode};
use crate::tools::workspace::Workspace;

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum Profile {
    Fast,
    Slow,
    Failed,
}
impl Profile {
    const fn delay(self) -> Duration {
        match self {
            Self::Fast => Duration::from_millis(10),
            Self::Slow | Self::Failed => Duration::from_millis(750),
        }
    }
}

struct Measurement {
    profile: Profile,
    start: Instant,
    active: usize,
    peak: usize,
    requests: BTreeMap<&'static str, usize>,
    first_call: Option<Duration>,
}
impl Measurement {
    fn new(profile: Profile) -> Self {
        Self {
            profile,
            start: Instant::now(),
            active: 0,
            peak: 0,
            requests: BTreeMap::new(),
            first_call: None,
        }
    }
}
type Meter = Arc<Mutex<Measurement>>;

#[derive(Clone)]
struct Peer {
    tool: String,
    meter: Meter,
}
impl Peer {
    async fn phase(&self, phase: &'static str) -> Profile {
        let profile = {
            let mut meter = self.meter.lock().unwrap();
            meter.active += 1;
            meter.peak = meter.peak.max(meter.active);
            *meter.requests.entry(phase).or_default() += 1;
            meter.profile
        };
        tokio::time::sleep(profile.delay()).await;
        self.meter.lock().unwrap().active -= 1;
        profile
    }
}
impl ServerHandler for Peer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
    }

    async fn discover(
        &self,
        _context: RequestContext<RoleServer>,
    ) -> Result<DiscoverResult, rmcp::ErrorData> {
        self.phase("handshake").await;
        Ok(DiscoverResult::from_server_info(
            crate::tools::mcp::supported_protocol_versions(),
            self.get_info(),
        ))
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, rmcp::ErrorData> {
        if matches!(self.phase("tools/list").await, Profile::Failed) {
            return Err(rmcp::ErrorData::internal_error(
                "controlled list failure",
                None,
            ));
        }
        Ok(ListToolsResult {
            tools: vec![crate::tools::mcp::fixture::fixture_tool_named(&self.tool)],
            ..Default::default()
        })
    }

    fn call_tool(
        &self,
        _request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<CallToolResponse, rmcp::ErrorData>> + Send {
        let mut meter = self.meter.lock().unwrap();
        *meter.requests.entry("tools/call").or_default() += 1;
        let elapsed = meter.start.elapsed();
        meter.first_call.get_or_insert(elapsed);
        std::future::ready(Ok(CallToolResult::success(Vec::new()).into()))
    }
}

struct HttpServer {
    binding: McpServerBinding,
    cancellation: tokio_util::sync::CancellationToken,
    task: tokio::task::JoinHandle<()>,
}
impl HttpServer {
    async fn start(tool: String, meter: Meter) -> Self {
        use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
        use rmcp::transport::streamable_http_server::{
            StreamableHttpServerConfig, StreamableHttpService,
        };
        let cancellation = tokio_util::sync::CancellationToken::new();
        let mut config = StreamableHttpServerConfig::default();
        config.cancellation_token = cancellation.child_token();
        config.sse_keep_alive = None;
        let peer = Peer { tool, meter };
        let service = StreamableHttpService::<Peer, LocalSessionManager>::new(
            move || Ok(peer.clone()),
            Arc::default(),
            config,
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            axum::serve(listener, axum::Router::new().nest_service("/mcp", service))
                .await
                .unwrap();
        });
        Self {
            binding: McpServerBinding {
                credentials: crate::credentials::SourceCredentials::default(),
                transport: McpTransportConfig::StreamableHttp {
                    endpoint: format!("http://{address}/mcp"),
                    headers: BTreeMap::new(),
                },
                policy: crate::tools::types::ToolInvocationPolicy::default(),
            },
            cancellation,
            task,
        }
    }
    async fn shutdown(self) {
        self.cancellation.cancel();
        self.task.abort();
        let _ = self.task.await;
    }
}

async fn prepare_initial(
    owner: &CapabilityCoordinator,
) -> Result<super::PreparedCapabilityCandidate, super::CapabilityPreparationError> {
    owner
        .prepare_startup_candidate(
            crate::skills::SkillDiscoveryOutcome::default(),
            BTreeSet::new(),
        )
        .await
}

fn coordinator(root: &std::path::Path, bindings: McpServerBindings) -> CapabilityCoordinator {
    let workspace_root = root.join("workspace");
    std::fs::create_dir_all(&workspace_root).unwrap();
    let sources: Vec<_> = bindings.keys().cloned().map(ToolSourceId::Mcp).collect();
    let mut activation = AgentActivation::default();
    activation.profile.tools.sources = sources
        .iter()
        .cloned()
        .map(|source| {
            (
                source,
                crate::capabilities::selection::SourceToolSelection::All,
            )
        })
        .collect();
    CapabilityCoordinator::new(CapabilityCoordinatorConfig {
        source_demand: crate::capabilities::source::ToolSourceDemand::new(
            sources,
            crate::runtime::resources::ManagedPythonCatalog::default(),
        ),
        conversation_id: ConversationId::new("conv_9f19ac05-7a2a-7707-beb8-6dcb7c9070e8"),
        workspace: Workspace::new(&workspace_root).unwrap(),
        base_tool_registry: Arc::new(ToolRegistry::new()),
        extension_tools: crate::extensions::ExtensionToolPlane::none(),
        agent_activation: activation,
        skill_discovery: crate::skills::SkillDiscoveryConfig::default(),
        mcp_servers: bindings,
        base_environment: crate::tools::environment::ToolEnvironment::new(),
        environment_store_root: root.join("environments"),
    })
    .unwrap()
}

struct NoProgress;
impl ProgressReporter for NoProgress {
    fn report(&self, _progress: crate::tools::types::ToolProgress) {}
}

async fn first_use(registry: &ToolRegistry, runtime: &ConversationToolRuntime) -> Option<f64> {
    let definition = registry.definitions().into_iter().next()?;
    let executor = registry.executor(&definition.id);
    let started = Instant::now();
    let _result = executor
        .start(
            ToolInvocation {
                id: ToolInvocationId::Agent {
                    call_id: ToolCallId::new("startup-measurement"),
                },
                tool_id: definition.id,
                tool_name: definition.name,
                mode: ToolInvocationMode::Foreground,
                arguments: serde_json::json!({}),
            },
            ToolExecutionContext::new(
                runtime.conversation_id(),
                None,
                crate::runtime::ExecutionCancellation::detached(
                    crate::runtime::CancellationSignal::new(),
                    crate::runtime::types::CancellationReason::UserRequested,
                ),
                runtime.workspace(),
                &NoProgress,
                runtime.artifacts(),
                runtime.tool_output(),
                runtime.environment(),
            ),
        )
        .completion
        .await;
    Some(milliseconds(started.elapsed()))
}

fn milliseconds(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

#[derive(Serialize)]
struct Row {
    profile: Profile,
    servers: usize,
    warm: bool,
    cache_eligible_hits: usize,
    cache_lookups: usize,
    warmup_requests: BTreeMap<&'static str, usize>,
    composition_settled_ms: f64,
    composition_ready_ms: Option<f64>,
    composition_error: Option<String>,
    discovery_settled_ms: f64,
    first_capability_ready_ms: Option<f64>,
    capability_ready_ms: Option<f64>,
    first_tool_dispatch_ms: Option<f64>,
    first_use_elapsed_ms: Option<f64>,
    initial_tools: usize,
    initial_connecting_sources: usize,
    initial_cached_sources: usize,
    final_ready_sources: usize,
    refresh_succeeded: bool,
    requests: BTreeMap<&'static str, usize>,
    peak_in_flight_phases: usize,
    spawned_server_processes: usize,
}

#[allow(
    clippy::too_many_lines,
    reason = "one measurement keeps timestamp and cleanup ordering explicit"
)]
async fn measure(servers: usize, profile: Profile, warm: bool) -> Row {
    let root = tempfile::tempdir().unwrap();
    let meter = Arc::new(Mutex::new(Measurement::new(Profile::Fast)));
    let mut fixtures = Vec::new();
    let mut bindings = BTreeMap::new();
    for index in 0..servers {
        let name = format!("startup-{index:02}");
        let server = HttpServer::start(name.clone(), meter.clone()).await;
        bindings.insert(McpServerId::new(name), server.binding.clone());
        fixtures.push(server);
    }
    if warm {
        let seed = coordinator(root.path(), bindings.clone());
        let candidate = seed.prepare_candidate().await.unwrap();
        seed.commit(candidate).unwrap();
        seed.drain_conversation_owned().await.unwrap();
    }
    let warmup_requests = meter.lock().unwrap().requests.clone();
    let owner = coordinator(root.path(), bindings.clone());
    let workspace = Workspace::new(root.path().join("workspace")).unwrap();
    let cache_eligible_hits = bindings
        .iter()
        .filter(|(id, binding)| {
            CatalogCacheKey::for_binding(id, binding, &workspace)
                .and_then(|key| McpCatalogCache::process().lookup(&key))
                .is_some()
        })
        .count();
    assert_eq!(cache_eligible_hits, if warm { servers } else { 0 });
    let tool_runtime = ConversationToolRuntime::new(
        ConversationId::new("conv_9f19ac05-7a2a-7707-beb8-6dcb7c9070e8"),
        workspace.root(),
        root.path().join("artifacts"),
    )
    .unwrap();
    *meter.lock().unwrap() = Measurement::new(profile);
    let start = meter.lock().unwrap().start;
    let prepared = prepare_initial(&owner).await;
    let composition_settled_ms = milliseconds(start.elapsed());
    let candidate = match prepared {
        Ok(candidate) => candidate,
        Err(error) => {
            assert!(owner.settle_failed_startup().await.is_empty());
            for server in fixtures {
                server.shutdown().await;
            }
            let measurement = meter.lock().unwrap();
            assert_eq!(measurement.active, 0);
            return Row {
                profile,
                servers,
                warm,
                cache_eligible_hits,
                cache_lookups: servers,
                warmup_requests,
                composition_settled_ms,
                composition_ready_ms: None,
                composition_error: Some(error.to_string()),
                discovery_settled_ms: composition_settled_ms,
                first_capability_ready_ms: None,
                capability_ready_ms: None,
                first_tool_dispatch_ms: None,
                first_use_elapsed_ms: None,
                initial_tools: 0,
                initial_connecting_sources: 0,
                initial_cached_sources: 0,
                final_ready_sources: 0,
                refresh_succeeded: false,
                requests: measurement.requests.clone(),
                peak_in_flight_phases: measurement.peak,
                spawned_server_processes: 0,
            };
        }
    };
    let initial_connecting_sources = candidate
        .availability()
        .values()
        .filter(|state| matches!(state, CapabilitySourceState::Connecting { .. }))
        .count();
    let initial_cached_sources = candidate
        .availability()
        .values()
        .filter(|state| matches!(state, CapabilitySourceState::Connecting { cached: true }))
        .count();
    let snapshot = owner.commit(candidate).unwrap();
    let lifecycle = owner.preparation_lifecycle().unwrap();
    assert!(lifecycle.activate());
    let composition_ready_ms = milliseconds(start.elapsed());
    let initial_tools = snapshot.tool_registry().definitions().len();
    let initially_ready = owner
        .availability()
        .values()
        .filter(|state| matches!(state, CapabilitySourceState::Ready))
        .count();
    let readiness = owner.startup_readiness();
    let settle = async {
        let mut refresh_succeeded = true;
        let mut first_ready = (initially_ready > 0).then_some(composition_ready_ms);
        let mut all_ready = (initially_ready == servers).then_some(composition_ready_ms);
        let mut late_first_use = None;
        if let Some(mut readiness) = readiness {
            loop {
                let done = readiness.borrow_and_update().done;
                // The native readiness candidate captures already-owned terminal
                // facts; no new discovery waits for a pending sibling.
                if owner.has_startup_updates() {
                    let inputs = owner.inner.resource_inputs.lock().unwrap().clone();
                    match owner
                        .prepare_ready_captured_inputs(
                            inputs,
                            &crate::runtime::cancellation::CancellationSignal::new(),
                        )
                        .await
                    {
                        Ok(candidate) => {
                            owner.commit(candidate).unwrap();
                        }
                        Err(_) => refresh_succeeded = false,
                    }
                }
                let ready = owner
                    .availability()
                    .values()
                    .filter(|state| matches!(state, CapabilitySourceState::Ready))
                    .count();
                if ready > 0 && first_ready.is_none() {
                    first_ready = Some(milliseconds(start.elapsed()));
                }
                if ready == servers && all_ready.is_none() {
                    all_ready = Some(milliseconds(start.elapsed()));
                }
                if initial_tools == 0 && late_first_use.is_none() {
                    let published = owner.current_snapshot();
                    late_first_use = first_use(published.tool_registry(), &tool_runtime).await;
                }
                if done {
                    break;
                }
                readiness.changed().await.unwrap();
            }
        }
        (
            milliseconds(start.elapsed()),
            refresh_succeeded,
            first_ready,
            all_ready,
            late_first_use,
        )
    };
    let (
        (
            discovery_settled_ms,
            refresh_succeeded,
            first_capability_ready_ms,
            capability_ready_ms,
            late_first_use,
        ),
        first_use_elapsed_ms,
    ) = tokio::join!(settle, first_use(snapshot.tool_registry(), &tool_runtime));
    let first_use_elapsed_ms = first_use_elapsed_ms.or(late_first_use);
    let final_ready_sources = owner
        .availability()
        .values()
        .filter(|state| matches!(state, CapabilitySourceState::Ready))
        .count();
    drop(snapshot);
    assert!(lifecycle.begin_drain());
    owner.drain_conversation_owned().await.unwrap();
    for server in fixtures {
        server.shutdown().await;
    }
    let measurement = meter.lock().unwrap();
    assert_eq!(measurement.active, 0);
    assert!(measurement.peak <= 4);
    assert_eq!(
        measurement.requests.get("tools/call").copied().unwrap_or(0),
        usize::from(servers > 0 && !matches!(profile, Profile::Failed))
    );
    Row {
        profile,
        servers,
        warm,
        cache_eligible_hits,
        cache_lookups: servers,
        warmup_requests,
        composition_settled_ms,
        composition_ready_ms: Some(composition_ready_ms),
        composition_error: None,
        discovery_settled_ms,
        first_capability_ready_ms,
        capability_ready_ms,
        first_tool_dispatch_ms: measurement.first_call.map(milliseconds),
        first_use_elapsed_ms,
        initial_tools,
        initial_connecting_sources,
        initial_cached_sources,
        final_ready_sources,
        refresh_succeeded,
        requests: measurement.requests.clone(),
        peak_in_flight_phases: measurement.peak,
        spawned_server_processes: 0,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "controlled startup benchmark; run explicitly with --ignored --nocapture"]
async fn optional_mcp_startup_timing() {
    let mut rows = Vec::new();
    for profile in [Profile::Fast, Profile::Slow, Profile::Failed] {
        for servers in [0, 1, 4, 8] {
            for warm in [false, true] {
                let row =
                    tokio::time::timeout(Duration::from_secs(30), measure(servers, profile, warm))
                        .await
                        .expect("startup timing row settles within the fixture deadline");
                println!("{}", serde_json::to_string(&row).unwrap());
                rows.push(row);
            }
        }
    }
    let evidence = serde_json::json!({
        "model_request_measurement": "composition_ready_ms is the completed initial candidate/commit boundary; no provider request is emitted by this fixture",
        "transport": "real loopback Streamable HTTP; client and servers share one test process",
        "fast_phase_delay_ms": 10, "slow_and_failed_phase_delay_ms": 750,
        "failed_phase": "tools/list returns a protocol error after its controlled delay",
        "rows": rows,
    });
    println!("{}", serde_json::to_string_pretty(&evidence).unwrap());
    if let Some(path) = std::env::var_os("RUSTX_STARTUP_TIMING_JSON") {
        std::fs::write(path, serde_json::to_vec_pretty(&evidence).unwrap()).unwrap();
    }
}

#[path = "performance_startup_model.rs"]
mod model;
