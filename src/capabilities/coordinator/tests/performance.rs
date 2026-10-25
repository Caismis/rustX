//! Opt-in, controlled-delay measurements of the real coordinator (Issue #498).
//! Run the identical fixture against the base and head; wall-clock values are
//! evidence only. Concurrency and exact request counts describe the work.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::future::BoxFuture;
use rmcp::model::{DiscoverResult, ListToolsResult, PaginatedRequestParams, ServerInfo};
use rmcp::service::RequestContext;
use rmcp::{RoleServer, ServerHandler};
use serde::Serialize;

use super::super::{CapabilityCoordinator, CapabilityCoordinatorConfig};
use crate::capabilities::selection::SourceToolSelection;
use crate::capabilities::{AgentActivation, CapabilitySourceState, ToolSourceId};
use crate::runtime::identity::{ConversationId, McpServerId};
use crate::runtime::process_runner::{
    CapturedProcessResult, ProcessOutcomeIntent, RunnerTestControl, SupervisedCommandSpec,
    SupervisedProcessRunner,
};
use crate::runtime::resources::ManagedPythonCatalog;
use crate::tools::environment::ToolEnvironment;
use crate::tools::executor::ToolRegistry;
use crate::tools::mcp::{McpServerBinding, McpServerBindings, McpTransportConfig};
use crate::tools::python::{PythonToolStore, discover_package};
use crate::tools::workspace::Workspace;

const DELAY: Duration = Duration::from_millis(100);

#[derive(Default)]
struct Measurement {
    active: usize,
    peak: usize,
    first_start: Option<Instant>,
    last_end: Option<Instant>,
    phases: BTreeMap<&'static str, Vec<f64>>,
}

type Meter = Arc<Mutex<Measurement>>;

async fn measured_delay(meter: &Meter, phase: &'static str) {
    let start = Instant::now();
    {
        let mut measurement = meter.lock().unwrap();
        measurement.active += 1;
        measurement.peak = measurement.peak.max(measurement.active);
        measurement.first_start.get_or_insert(start);
    }
    tokio::time::sleep(DELAY).await;
    let end = Instant::now();
    let mut measurement = meter.lock().unwrap();
    measurement.active -= 1;
    measurement.last_end = Some(end);
    measurement
        .phases
        .entry(phase)
        .or_default()
        .push(milliseconds(end.duration_since(start)));
}

#[derive(Serialize)]
struct TimingRow {
    kind: &'static str,
    sources: usize,
    composition_ms: f64,
    /// First controlled phase entry through last phase completion. Scheduling
    /// gaps may include earlier packages' synthesized connection attempts.
    controlled_work_span_ms: f64,
    peak_in_flight_phases: usize,
    phase_request_counts: BTreeMap<&'static str, usize>,
    phase_durations_ms: BTreeMap<&'static str, Vec<f64>>,
}

fn milliseconds(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

fn row(kind: &'static str, sources: usize, elapsed: Duration, meter: &Meter) -> TimingRow {
    let measurement = meter.lock().unwrap();
    assert_eq!(measurement.active, 0);
    TimingRow {
        kind,
        sources,
        composition_ms: milliseconds(elapsed),
        controlled_work_span_ms: milliseconds(
            measurement
                .last_end
                .unwrap()
                .duration_since(measurement.first_start.unwrap()),
        ),
        peak_in_flight_phases: measurement.peak,
        phase_request_counts: measurement
            .phases
            .iter()
            .map(|(&phase, durations)| (phase, durations.len()))
            .collect(),
        phase_durations_ms: measurement.phases.clone(),
    }
}

#[derive(Clone)]
struct DelayedServer {
    name: String,
    meter: Meter,
}

impl ServerHandler for DelayedServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(
            rmcp::model::ServerCapabilities::builder()
                .enable_tools()
                .build(),
        )
    }

    async fn discover(
        &self,
        _context: RequestContext<RoleServer>,
    ) -> Result<DiscoverResult, rmcp::ErrorData> {
        measured_delay(&self.meter, "handshake").await;
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
        measured_delay(&self.meter, "tools/list").await;
        Ok(ListToolsResult {
            tools: vec![crate::tools::mcp::fixture::fixture_tool_named(&self.name)],
            ..Default::default()
        })
    }
}

struct HttpServer {
    binding: McpServerBinding,
    cancellation: tokio_util::sync::CancellationToken,
    task: tokio::task::JoinHandle<()>,
}

impl HttpServer {
    async fn start(name: String, meter: Meter) -> Self {
        use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
        use rmcp::transport::streamable_http_server::{
            StreamableHttpServerConfig, StreamableHttpService,
        };

        let cancellation = tokio_util::sync::CancellationToken::new();
        let mut config = StreamableHttpServerConfig::default();
        config.cancellation_token = cancellation.child_token();
        config.sse_keep_alive = None;
        let handler = DelayedServer { name, meter };
        let service = StreamableHttpService::<DelayedServer, LocalSessionManager>::new(
            move || Ok(handler.clone()),
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

fn coordinator(
    dir: &std::path::Path,
    bindings: McpServerBindings,
    packages: ManagedPythonCatalog,
) -> CapabilityCoordinator {
    let workspace_root = dir.join("workspace");
    std::fs::create_dir_all(&workspace_root).unwrap();
    let sources = bindings
        .keys()
        .cloned()
        .map(ToolSourceId::Mcp)
        .chain(packages.packages().keys().cloned())
        .collect::<Vec<_>>();
    let mut activation = AgentActivation::default();
    activation.profile.tools.sources = sources
        .iter()
        // Python rows measure optional admitted builds and intentionally fail
        // their synthesized connection; no unavailable tool is activated.
        .filter(|source| matches!(source, ToolSourceId::Mcp(_)))
        .cloned()
        .map(|source| (source, SourceToolSelection::All))
        .collect();
    CapabilityCoordinator::new(CapabilityCoordinatorConfig {
        source_demand: crate::capabilities::source::ToolSourceDemand::new(sources, packages),
        conversation_id: ConversationId::new("conv_8599f7eb-67bc-7fe4-9327-c59073681c64"),
        workspace: Workspace::new(&workspace_root).unwrap(),
        base_tool_registry: Arc::new(ToolRegistry::new()),
        extension_tools: crate::extensions::ExtensionToolPlane::none(),
        agent_activation: activation,
        skill_discovery: crate::skills::SkillDiscoveryConfig::workspace_root(
            workspace_root.join(".agents/skills"),
        ),
        mcp_servers: bindings,
        base_environment: ToolEnvironment::new(),
        environment_store_root: dir.join("environment-store"),
    })
    .unwrap()
}

async fn mcp_row(sources: usize) -> TimingRow {
    let dir = tempfile::tempdir().unwrap();
    let meter = Meter::default();
    let mut servers = Vec::new();
    let mut bindings = BTreeMap::new();
    for index in 0..sources {
        let name = format!("server-{index:02}");
        let server = HttpServer::start(name.clone(), meter.clone()).await;
        bindings.insert(McpServerId::new(name), server.binding.clone());
        servers.push(server);
    }
    let coordinator = coordinator(dir.path(), bindings, ManagedPythonCatalog::default());
    let start = Instant::now();
    let candidate = coordinator.prepare_candidate().await.unwrap();
    assert!(
        candidate
            .availability()
            .values()
            .all(|state| *state == CapabilitySourceState::Ready)
    );
    let snapshot = coordinator.commit(candidate).unwrap();
    let elapsed = start.elapsed();
    assert_eq!(snapshot.tool_registry().definitions().len(), sources);
    coordinator.drain_conversation_owned().await.unwrap();
    for server in servers {
        server.shutdown().await;
    }
    let result = row("mcp_http", sources, elapsed, &meter);
    assert_eq!(result.phase_durations_ms["handshake"].len(), sources);
    assert_eq!(result.phase_durations_ms["tools/list"].len(), sources);
    result
}

struct DelayedPythonRunner {
    meter: Meter,
}

impl SupervisedProcessRunner for DelayedPythonRunner {
    fn run(
        &self,
        spec: SupervisedCommandSpec,
        _control: Option<RunnerTestControl>,
    ) -> BoxFuture<'_, Result<CapturedProcessResult, String>> {
        Box::pin(async move {
            let stdout = if spec.command.contains("--version") {
                if spec.command.contains("python3") {
                    b"Python 3.12.13\n".to_vec()
                } else {
                    b"uv 0.11.12\n".to_vec()
                }
            } else {
                let phase = if spec.command.contains(" lock ") {
                    "uv lock"
                } else {
                    "uv sync"
                };
                measured_delay(&self.meter, phase).await;
                if phase == "uv sync" {
                    // Build timing is measured separately from MCP. The
                    // synthesized binding deliberately fails at exec, as in
                    // the existing deterministic managed-Python fixtures.
                    let program = spec.cwd.join("venv").join(if cfg!(windows) {
                        "Scripts/python.exe"
                    } else {
                        "bin/python"
                    });
                    std::fs::create_dir_all(program.parent().unwrap()).unwrap();
                    std::fs::write(program, b"non-executable benchmark interpreter\n").unwrap();
                }
                Vec::new()
            };
            Ok(CapturedProcessResult {
                exit_code: Some(0),
                intent: ProcessOutcomeIntent::Completed,
                stdout,
                stderr: Vec::new(),
            })
        })
    }
}

async fn python_row(sources: usize) -> TimingRow {
    let dir = tempfile::tempdir().unwrap();
    let mut packages = BTreeMap::new();
    for index in 0..sources {
        let name = format!("package-{index:02}");
        let root = dir.path().join("workspace/.agents/tools").join(&name);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("server.py"),
            "from fastmcp import FastMCP\nmcp = FastMCP('benchmark')\n",
        )
        .unwrap();
        std::fs::write(root.join("requirements.txt"), "# empty\n").unwrap();
        packages.insert(
            ToolSourceId::ManagedPython(name.clone()),
            discover_package(&root, &name),
        );
    }
    let coordinator = coordinator(
        dir.path(),
        BTreeMap::new(),
        ManagedPythonCatalog::new(packages),
    );
    let meter = Meter::default();
    coordinator.install_python_store(
        PythonToolStore::with_binaries_and_runner(
            dir.path().join("environment-store/python-tools"),
            PathBuf::from("/benchmark/uv"),
            PathBuf::from("/benchmark/python3"),
            Arc::new(DelayedPythonRunner {
                meter: meter.clone(),
            }),
        )
        .unwrap(),
    );
    let start = Instant::now();
    let candidate = coordinator.prepare_candidate().await.unwrap();
    assert_eq!(candidate.availability().len(), sources);
    assert!(candidate.availability().values().all(|state| matches!(state, CapabilitySourceState::Unavailable { reason } if reason.contains("MCP discovery failed"))));
    coordinator.commit(candidate).unwrap();
    let elapsed = start.elapsed();
    coordinator.drain_conversation_owned().await.unwrap();
    let result = row("python_scripted_build", sources, elapsed, &meter);
    assert_eq!(result.phase_durations_ms["uv lock"].len(), sources);
    assert_eq!(result.phase_durations_ms["uv sync"].len(), sources);
    result
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "controlled-delay benchmark; run explicitly with --ignored --nocapture"]
async fn source_preparation_timing() {
    let mut rows = Vec::new();
    for sources in [1, 4, 8] {
        rows.push(mcp_row(sources).await);
    }
    for sources in [1, 4, 8] {
        rows.push(python_row(sources).await);
    }
    let evidence = serde_json::json!({
        "controlled_phase_delay_ms": DELAY.as_millis(),
        "rows": rows,
    });
    println!("{}", serde_json::to_string_pretty(&evidence).unwrap());
    if let Some(path) = std::env::var_os("RUSTX_SOURCE_TIMING_JSON") {
        std::fs::write(path, serde_json::to_vec_pretty(&evidence).unwrap()).unwrap();
    }
}
