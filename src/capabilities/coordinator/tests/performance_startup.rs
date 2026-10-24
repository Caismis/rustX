//! Reproducible live-startup measurement support. The companion module measures
//! a real `ConversationRuntime` request and real loopback HTTP effects. Controlled
//! delays are benchmark inputs, never synchronization proofs for correctness.

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
            Self::Slow | Self::Failed => Duration::from_millis(1500),
        }
    }
}

struct Measurement {
    profile: Profile,
    start: Instant,
    active: usize,
    peak: usize,
    requests: BTreeMap<&'static str, usize>,
    http_requests: BTreeMap<String, usize>,
    server_handler_instances: usize,
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
            http_requests: BTreeMap::new(),
            server_handler_instances: 0,
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
        let peer = Peer {
            tool,
            meter: meter.clone(),
        };
        let session_meter = meter.clone();
        let service = StreamableHttpService::<Peer, LocalSessionManager>::new(
            move || {
                session_meter.lock().unwrap().server_handler_instances += 1;
                Ok(peer.clone())
            },
            Arc::default(),
            config,
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app =
            axum::Router::new()
                .nest_service("/mcp", service)
                .layer(axum::middleware::from_fn(
                    move |request: axum::extract::Request, next: axum::middleware::Next| {
                        let meter = meter.clone();
                        async move {
                            *meter
                                .lock()
                                .unwrap()
                                .http_requests
                                .entry(request.method().to_string())
                                .or_default() += 1;
                            next.run(request).await
                        }
                    },
                ));
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
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
    let deadline = crate::capabilities::McpStartupDeadline::default();
    deadline.arm(tokio::time::Instant::now() + Duration::from_secs(15));
    let result = owner
        .prepare_startup_candidate(
            crate::skills::SkillDiscoveryOutcome::default(),
            BTreeSet::new(),
            deadline,
            &crate::runtime::CancellationSignal::new(),
        )
        .await;
    assert!(
        result.is_ok(),
        "optional failed MCP sources must not reject the Root: {:?}",
        result.as_ref().err()
    );
    result
}

fn coordinator(
    root: &std::path::Path,
    bindings: McpServerBindings,
    conversation: &str,
) -> CapabilityCoordinator {
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
        conversation_id: ConversationId::new(conversation),
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

/// Exercise the captured native executor after the measured Attempt settles.
/// This is an external MCP effect, not an additional model-generated step.
async fn first_use(registry: &ToolRegistry, runtime: &ConversationToolRuntime) -> Option<f64> {
    let definition = registry.definitions().into_iter().next()?;
    let executor = registry.executor(&definition.id);
    let started = Instant::now();
    let result = executor
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
    assert!(
        matches!(
            result.status,
            crate::tools::types::ToolExecutionStatus::Success
        ),
        "{result:?}"
    );
    Some(milliseconds(started.elapsed()))
}

fn milliseconds(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

#[path = "performance_startup_model.rs"]
mod model;
