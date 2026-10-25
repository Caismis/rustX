//! Native runtime catalog publication is isolated from pending authored configuration.
//! Live schema validation and provider gates determine every ordering.
use super::*;
use crate::runtime::identity::McpServerId;
use rmcp::RoleServer;
use rmcp::ServerHandler;
use rmcp::model::{
    DiscoverResult, ListToolsResult, PaginatedRequestParams, ProtocolVersion, ServerCapabilities,
    ServerInfo,
};
use rmcp::service::RequestContext;
use std::sync::atomic::AtomicBool;

mod draft_prewarm;
mod profile_controls;

const TOOL: &str = "startup_echo";

#[derive(Clone)]
struct StartupServer {
    gate: Arc<super::super::AsyncGate>,
    schema_kind: Arc<std::sync::Mutex<&'static str>>,
    lists: Arc<AtomicUsize>,
    discovers: Arc<AtomicUsize>,
    completed: tokio::sync::watch::Sender<usize>,
    fail: Arc<AtomicBool>,
    calls: Arc<AtomicUsize>,
    name: Arc<std::sync::Mutex<&'static str>>,
}

impl ServerHandler for StartupServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
    }

    fn supported_protocol_versions(&self) -> std::borrow::Cow<'static, [ProtocolVersion]> {
        std::borrow::Cow::Owned(vec![ProtocolVersion::V_2026_07_28])
    }

    fn discover(
        &self,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<DiscoverResult, rmcp::ErrorData>> {
        self.discovers.fetch_add(1, Ordering::SeqCst);
        std::future::ready(Ok(DiscoverResult::from_server_info(
            vec![ProtocolVersion::V_2026_07_28],
            self.get_info(),
        )))
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, rmcp::ErrorData> {
        self.lists.fetch_add(1, Ordering::SeqCst);
        self.gate.park().await;
        if self.fail.load(Ordering::SeqCst) {
            return Err(rmcp::ErrorData::internal_error(
                "startup fixture unavailable",
                None,
            ));
        }
        let mut tool = crate::tools::mcp::fixture::fixture_tool_named(*self.name.lock().unwrap());
        let kind = *self.schema_kind.lock().unwrap();
        Arc::make_mut(&mut tool.input_schema)
            .insert("properties".into(), serde_json::json!({"id":{"type":kind}}));
        self.completed.send_modify(|completed| *completed += 1);
        Ok(ListToolsResult {
            tools: vec![tool],
            ..Default::default()
        })
    }

    fn call_tool(
        &self,
        _request: rmcp::model::CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<rmcp::model::CallToolResponse, rmcp::ErrorData>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        std::future::ready(Ok(rmcp::model::CallToolResult::success(vec![]).into()))
    }
}

struct McpFixture {
    endpoint: String,
    control: StartupServer,
    task: tokio::task::JoinHandle<()>,
}

impl McpFixture {
    async fn start() -> Self {
        Self::named(TOOL).await
    }

    async fn named(name: &'static str) -> Self {
        use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
        use rmcp::transport::streamable_http_server::{
            StreamableHttpServerConfig, StreamableHttpService,
        };
        let control = StartupServer {
            gate: Arc::new(super::super::AsyncGate::default()),
            schema_kind: Arc::new(std::sync::Mutex::new("string")),
            lists: Arc::new(AtomicUsize::new(0)),
            discovers: Arc::new(AtomicUsize::new(0)),
            completed: tokio::sync::watch::channel(0).0,
            fail: Arc::new(AtomicBool::new(false)),
            calls: Arc::new(AtomicUsize::new(0)),
            name: Arc::new(std::sync::Mutex::new(name)),
        };
        let handler = control.clone();
        let mut config = StreamableHttpServerConfig::default();
        config.sse_keep_alive = None;
        let service = StreamableHttpService::<StartupServer, LocalSessionManager>::new(
            move || Ok(handler.clone()),
            Arc::default(),
            config,
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}/mcp", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            axum::serve(listener, axum::Router::new().nest_service("/mcp", service))
                .await
                .unwrap();
        });
        Self {
            endpoint,
            control,
            task,
        }
    }

    async fn close(self) {
        self.task.abort();
        let _ = self.task.await;
    }
}

async fn create(fixture: &Fixture) -> SessionSnapshot {
    let created = fixture
        .manager
        .create_session(SessionPersistentState {
            cwd: fixture.workspaces[0].clone(),
            model: None,
        })
        .await
        .unwrap();
    fixture
        .manager
        .sessions
        .rename_session(&created.session.id, "MCP readiness fixture")
        .await
        .unwrap()
}

fn tool_schema(body: &str) -> Option<String> {
    let request: serde_json::Value = serde_json::from_str(body).unwrap();
    request["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|tool| tool["function"]["name"] == TOOL)
        .map(|tool| {
            tool["function"]["parameters"]["properties"]["id"]["type"]
                .as_str()
                .unwrap()
                .to_owned()
        })
}

async fn execute_current_tool(
    runtime: &crate::runtime::ConversationRuntime,
    arguments: serde_json::Value,
) -> crate::tools::types::ToolExecutionResult {
    use crate::tools::executor::{ProgressReporter, ToolExecutionContext};
    use crate::tools::types::{ToolInvocation, ToolInvocationId, ToolInvocationMode};

    struct Silent;
    impl ProgressReporter for Silent {
        fn report(&self, _: crate::tools::types::ToolProgress) {}
    }
    let resources = runtime.runtime_resources();
    let registration = resources
        .capability()
        .tool_registry()
        .registrations()
        .into_iter()
        .find(|entry| entry.definition.name == TOOL)
        .unwrap();
    let tools = runtime.tool_runtime();
    let context = ToolExecutionContext::new(
        tools.conversation_id(),
        None,
        crate::runtime::ExecutionCancellation::detached(
            crate::runtime::cancellation::CancellationSignal::new(),
            crate::runtime::types::CancellationReason::UserRequested,
        ),
        tools.workspace(),
        &Silent,
        tools.artifacts(),
        tools.tool_output(),
        tools.environment(),
    );
    registration
        .executor
        .start(
            ToolInvocation {
                id: ToolInvocationId::Agent {
                    call_id: crate::runtime::identity::ToolCallId::new("recovery-call"),
                },
                tool_id: registration.definition.id,
                tool_name: registration.definition.name,
                mode: ToolInvocationMode::Foreground,
                arguments,
            },
            context,
        )
        .completion
        .await
}

fn configure(fixture: &Fixture, sources: &[(&str, &McpFixture)]) {
    use std::fmt::Write as _;
    let workspace = &fixture.workspaces[0];
    std::fs::create_dir_all(workspace.join(".agents")).unwrap();
    let mut servers = String::new();
    for (id, server) in sources {
        writeln!(servers, "[mcp_servers.{id}]\nurl = {:?}", server.endpoint).unwrap();
    }
    std::fs::write(workspace.join(".agents/mcp.toml"), servers).unwrap();
    let config = workspace.join("rustx.toml");
    let mut document = std::fs::read_to_string(&config).unwrap();
    document.push_str("\n[agent.tools.sources]\n");
    for (id, _) in sources {
        writeln!(document, "{id} = \"all\"").unwrap();
    }
    std::fs::write(config, document).unwrap();
}

async fn wait_runtime_catalog(
    runtime: &crate::runtime::ConversationRuntime,
    publications: &mut tokio::sync::watch::Receiver<u64>,
    before: u64,
) {
    publications
        .wait_for(|revision| *revision > before)
        .await
        .unwrap();
    assert!(runtime.runtime_resources().revision().get() > 1);
}

async fn ready_rebased(
    fixture: &Fixture,
    session: &SessionSnapshot,
    runtime: &crate::runtime::ConversationRuntime,
) -> crate::local_runtime::configuration::application::AvailableConfiguration {
    let mut changes = fixture.manager.configuration_changes();
    loop {
        if let Some(candidate) = fixture
            .manager
            .configuration_application(&session.id)
            .and_then(|application| application.candidate)
            && candidate.expected_binding == runtime.configuration_cut().binding_revision
        {
            return candidate;
        }
        changes.changed().await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(clippy::too_many_lines)]
async fn live_schema_recovery_preserves_whole_attempt_and_pending_authored_intent() {
    Box::pin(bounded(async {
        let mcp = McpFixture::start().await;
        let replacement = McpFixture::named("unadopted_tool").await;
        let mut fixture = Fixture::with_session_count(Some("read"), 0).await;
        configure(&fixture, &[("late", &mcp)]);
        let session = create(&fixture).await;
        fixture.sessions.push(session.clone());
        fixture.manager.load(&session.id, None).await.unwrap();
        let runtime = fixture.manager.configuration_runtime(&session.id).unwrap();
        let frozen = runtime.runtime_resources();
        let mut publications = runtime.runtime_catalog_publications();
        let before = *publications.borrow();
        runtime
            .submit_inbound(input("request-A: frozen multi-step Attempt"))
            .unwrap();
        fixture.gates[0].wait_entered().await;
        let admitted = runtime.configuration_cut().admitted_attempt.unwrap();
        assert_eq!(
            tool_schema(&fixture.provider.request_bodies()[0]).as_deref(),
            Some("string")
        );

        // A real invalidation and first-use live check learns schema B. The
        // old frozen A call fails before the peer receives an effect.
        *mcp.control.schema_kind.lock().unwrap() = "integer";
        runtime
            .capability()
            .mcp_invalidation()
            .lock()
            .advance(&McpServerId::new("late"));
        assert!(matches!(
            execute_current_tool(&runtime, serde_json::json!({"id":"old"}))
                .await
                .status,
            crate::tools::types::ToolExecutionStatus::Failed { .. }
        ));
        assert_eq!(mcp.control.calls.load(Ordering::SeqCst), 0);
        assert_eq!(mcp.control.lists.load(Ordering::SeqCst), 2);
        assert_eq!(runtime.runtime_resources().revision(), frozen.revision());
        assert_eq!(
            runtime.configuration_cut().admitted_attempt.unwrap(),
            admitted
        );

        // Capturing a different source and instructions creates an authored
        // candidate, which live recovery must never pull into the runtime.
        std::fs::write(
            fixture.workspaces[0].join(".agents/mcp.toml"),
            format!("[mcp_servers.late]\nurl = {:?}\n", replacement.endpoint),
        )
        .unwrap();
        write(
            &fixture,
            0,
            ConfigMutation::Instructions {
                authored: Some("unadopted instructions".into()),
            },
        )
        .await;
        let pending = settled(&fixture, 0).await;
        assert!(pending.candidate.is_some());
        let selected = crate::model::session::SessionModelConfig::of(
            crate::model::catalog::ModelRef::parse("local/b").unwrap(),
        );
        fixture
            .manager
            .set_model(&session.id, selected.clone())
            .await
            .unwrap();
        let pending = settled(&fixture, 0).await;
        assert!(pending.candidate.is_some());
        assert_eq!(
            runtime.configuration_cut().admitted_attempt.unwrap(),
            admitted
        );

        let mut settlement = SettlementReceiptWait::capture(&runtime);
        fixture.gates[0].release();
        settlement.wait().await;
        wait_runtime_catalog(&runtime, &mut publications, before).await;
        let candidate = ready_rebased(&fixture, &session, &runtime).await;
        let requests = fixture.provider.request_bodies();
        assert_eq!(
            requests.len(),
            2,
            "tool proposal and continuation belong to one frozen Attempt"
        );
        for request in &requests {
            assert_eq!(tool_schema(request).as_deref(), Some("string"));
            assert!(!request.contains("unadopted instructions"));
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(request).unwrap()["model"],
                "a"
            );
        }
        assert_eq!(runtime.model_view().configured, selected);
        assert_ne!(runtime.runtime_resources().revision(), frozen.revision());
        assert_eq!(
            runtime
                .runtime_resources()
                .configuration()
                .unwrap()
                .config
                .agent
                .instructions,
            frozen.configuration().unwrap().config.agent.instructions
        );
        assert!(
            runtime
                .runtime_resources()
                .capability()
                .tool_registry()
                .definitions()
                .iter()
                .all(|tool| tool.name != "unadopted_tool")
        );

        runtime
            .submit_inbound(input("request-B: automatic schema publication"))
            .unwrap();
        fixture.gates[1].wait_entered().await;
        let requests = fixture.provider.request_bodies();
        assert_eq!(tool_schema(&requests[2]).as_deref(), Some("integer"));
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&requests[2]).unwrap()["model"],
            "b"
        );
        assert!(!requests[2].contains("unadopted instructions"));
        let mut settlement = SettlementReceiptWait::capture(&runtime);
        fixture.gates[1].release();
        settlement.wait().await;
        runtime.wait_for_configuration_admissions().await;
        fixture
            .manager
            .adopt_configuration(&session.id, &candidate.identity, candidate.expected_binding)
            .unwrap();
        assert_eq!(runtime.model_view().configured, selected);
        assert_eq!(
            runtime
                .runtime_resources()
                .configuration()
                .unwrap()
                .config
                .agent
                .instructions,
            "unadopted instructions"
        );
        assert!(
            runtime
                .runtime_resources()
                .capability()
                .tool_registry()
                .definitions()
                .iter()
                .any(|tool| tool.name == "unadopted_tool")
        );
        assert_eq!(
            mcp.control.lists.load(Ordering::SeqCst),
            2,
            "automatic publication never restarts completed live discovery"
        );
        fixture.close().await;
        mcp.close().await;
        replacement.close().await;
    }))
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn idle_runtime_catalog_publishes_before_the_next_attempt_without_input() {
    bounded(async {
        let mcp = McpFixture::start().await;
        let mut fixture = Fixture::with_session_count(None, 0).await;
        configure(&fixture, &[("late", &mcp)]);
        let session = create(&fixture).await;
        fixture.sessions.push(session.clone());
        fixture.manager.load(&session.id, None).await.unwrap();
        let runtime = fixture.manager.configuration_runtime(&session.id).unwrap();
        let mut publications = runtime.runtime_catalog_publications();
        let before = *publications.borrow();
        let history = runtime
            .tool_runtime()
            .durable_store()
            .read_events(None, 256)
            .unwrap()
            .events
            .len();
        *mcp.control.schema_kind.lock().unwrap() = "integer";
        runtime
            .capability()
            .mcp_invalidation()
            .lock()
            .advance(&McpServerId::new("late"));
        assert!(matches!(
            execute_current_tool(&runtime, serde_json::json!({"id":"old"}))
                .await
                .status,
            crate::tools::types::ToolExecutionStatus::Failed { .. }
        ));
        wait_runtime_catalog(&runtime, &mut publications, before).await;
        assert!(fixture.provider.request_bodies().is_empty());
        assert_eq!(
            runtime
                .tool_runtime()
                .durable_store()
                .read_events(None, 256)
                .unwrap()
                .events
                .len(),
            history
        );
        assert!(matches!(
            execute_current_tool(&runtime, serde_json::json!({"id":42}))
                .await
                .status,
            crate::tools::types::ToolExecutionStatus::Success
        ));
        assert_eq!(mcp.control.calls.load(Ordering::SeqCst), 1);
        runtime
            .submit_inbound(input("request-B: first request after live update"))
            .unwrap();
        fixture.gates[1].wait_entered().await;
        assert_eq!(
            tool_schema(&fixture.provider.request_bodies()[0]).as_deref(),
            Some("integer")
        );
        let mut settlement = SettlementReceiptWait::capture(&runtime);
        fixture.gates[1].release();
        settlement.wait().await;
        assert_eq!(mcp.control.lists.load(Ordering::SeqCst), 2);
        fixture.close().await;
        mcp.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unchanged_live_catalog_consumes_epoch_without_semantic_publication() {
    bounded(async {
        let mcp = McpFixture::start().await;
        let mut fixture = Fixture::with_session_count(None, 0).await;
        configure(&fixture, &[("late", &mcp)]);
        let session = create(&fixture).await;
        fixture.sessions.push(session.clone());
        fixture.manager.load(&session.id, None).await.unwrap();
        let runtime = fixture.manager.configuration_runtime(&session.id).unwrap();
        let original = runtime.configuration_cut();
        let publications = runtime.runtime_catalog_publications();
        let published = *publications.borrow();
        let mut observed = runtime.runtime_catalog_observations();
        let previous_observation = *observed.borrow();
        let history = runtime
            .tool_runtime()
            .durable_store()
            .read_events(None, 256)
            .unwrap()
            .events
            .len();
        runtime
            .capability()
            .mcp_invalidation()
            .lock()
            .advance(&McpServerId::new("late"));
        assert!(matches!(
            execute_current_tool(&runtime, serde_json::json!({"id":"unchanged"}))
                .await
                .status,
            crate::tools::types::ToolExecutionStatus::Success
        ));
        let discovered = runtime.capability().catalog_revision();
        assert!(
            discovered > previous_observation,
            "successful discovery has a new observation"
        );
        observed
            .wait_for(|revision| *revision >= discovered)
            .await
            .unwrap();
        let current = runtime.configuration_cut();
        assert_eq!(
            current.resources.capability().revision(),
            original.resources.capability().revision()
        );
        assert_eq!(current.resources.revision(), original.resources.revision());
        assert_eq!(current.binding_revision, original.binding_revision);
        assert_eq!(*publications.borrow(), published);
        assert!(!publications.has_changed().unwrap());
        assert!(fixture.provider.request_bodies().is_empty());
        assert_eq!(
            runtime
                .tool_runtime()
                .durable_store()
                .read_events(None, 256)
                .unwrap()
                .events
                .len(),
            history
        );
        assert_eq!(mcp.control.lists.load(Ordering::SeqCst), 2);
        assert_eq!(mcp.control.calls.load(Ordering::SeqCst), 1);
        fixture.close().await;
        mcp.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn failed_explicit_runtime_refresh_retains_last_good_without_automatic_retry() {
    bounded(async {
        let mcp = McpFixture::start().await;
        let mut fixture = Fixture::with_session_count(None, 0).await;
        configure(&fixture, &[("late", &mcp)]);
        let session = create(&fixture).await;
        fixture.sessions.push(session.clone());
        fixture.manager.load(&session.id, None).await.unwrap();
        let runtime = fixture.manager.configuration_runtime(&session.id).unwrap();
        let original = runtime.runtime_resources();
        mcp.control.fail.store(true, Ordering::SeqCst);
        assert!(runtime.refresh_runtime_catalogs().await.is_err());
        assert_eq!(mcp.control.lists.load(Ordering::SeqCst), 2);
        assert_eq!(
            runtime.runtime_resources().capability().revision(),
            original.capability().revision()
        );
        assert!(
            runtime
                .runtime_resources()
                .inspection()
                .resource_diagnostics
                .iter()
                .any(|diagnostic| diagnostic.field == "runtime.catalog")
        );
        assert!(fixture.provider.request_bodies().is_empty());
        write(
            &fixture,
            0,
            ConfigMutation::Instructions {
                authored: Some("new authored instructions".into()),
            },
        )
        .await;
        let authored = settled(&fixture, 0).await.candidate.unwrap();
        runtime.wait_for_configuration_admissions().await;
        fixture
            .manager
            .adopt_configuration(&session.id, &authored.identity, authored.expected_binding)
            .unwrap();
        let adopted = runtime.runtime_resources();
        assert_eq!(
            adopted.capability().tool_registry().definitions(),
            original.capability().tool_registry().definitions()
        );
        assert_eq!(
            adopted.capability().revision(),
            original.capability().revision()
        );
        let executor = |resources: &crate::runtime::resources::RuntimeResourceSnapshot| {
            resources
                .capability()
                .tool_registry()
                .registrations()
                .into_iter()
                .find(|registration| registration.definition.name == TOOL)
                .unwrap()
                .executor
        };
        assert!(Arc::ptr_eq(&executor(&adopted), &executor(&original)));
        assert!(
            adopted
                .inspection()
                .resource_diagnostics
                .iter()
                .any(|diagnostic| diagnostic.field == "runtime.catalog")
        );
        assert_eq!(
            mcp.control.lists.load(Ordering::SeqCst),
            2,
            "context preparation and adoption cannot retry discovery"
        );
        assert_eq!(mcp.control.calls.load(Ordering::SeqCst), 0);
        mcp.control.fail.store(false, Ordering::SeqCst);
        *mcp.control.schema_kind.lock().unwrap() = "integer";
        let mut publications = runtime.runtime_catalog_publications();
        let before = *publications.borrow();
        runtime.refresh_runtime_catalogs().await.unwrap();
        wait_runtime_catalog(&runtime, &mut publications, before).await;
        assert_eq!(mcp.control.lists.load(Ordering::SeqCst), 3);
        assert!(
            !runtime
                .runtime_resources()
                .inspection()
                .resource_diagnostics
                .iter()
                .any(|diagnostic| diagnostic.field == "runtime.catalog")
        );
        assert_eq!(
            runtime
                .runtime_resources()
                .configuration()
                .unwrap()
                .config
                .agent
                .instructions,
            "new authored instructions"
        );
        assert!(fixture.provider.request_bodies().is_empty());
        fixture.close().await;
        mcp.close().await;
    })
    .await;
}

#[tokio::test]
async fn first_submission_joins_unarmed_early_preparation_with_one_native_budget() {
    let mcp = McpFixture::start().await;
    let mut fixture = Fixture::with_session_count(None, 0).await;
    configure(&fixture, &[("late", &mcp)]);
    mcp.control.gate.arm();
    let session = create(&fixture).await;
    fixture.sessions.push(session.clone());
    let loading = fixture.load(0);
    tokio::time::timeout(
        std::time::Duration::from_secs(15),
        mcp.control.gate.entered(),
    )
    .await
    .unwrap();
    tokio::time::pause();
    tokio::time::advance(std::time::Duration::from_secs(60)).await;
    tokio::task::yield_now().await;
    assert!(
        !loading.is_finished(),
        "early preparation has no elapsed-time deadline"
    );
    assert!(fixture.provider.request_bodies().is_empty());
    tokio::time::resume();

    let connection = crate::app_server::connection::AppServerConnection::new(fixture.host.clone());
    super::super::protocol::initialize(&connection).await;
    assert!(matches!(
        super::super::protocol::call(
            &connection,
            700,
            crate::app_server::protocol::Method::SessionSubmissionIntent {
                session_id: session.id.clone(),
                node_id: session.active_node.clone(),
            }
        )
        .await,
        crate::app_server::protocol::MethodResult::SubmissionIntentAccepted {}
    ));
    tokio::time::pause();
    tokio::time::advance(std::time::Duration::from_secs(10)).await;
    tokio::task::yield_now().await;
    assert!(
        !loading.is_finished(),
        "the first native receipt still allows its remaining budget"
    );
    tokio::time::resume();
    mcp.control.gate.release();
    bounded(loading).await.unwrap().unwrap();
    let runtime = fixture.manager.configuration_runtime(&session.id).unwrap();
    runtime
        .submit_inbound(input("request-B: first submission after preparation"))
        .unwrap();
    fixture.gates[1].wait_entered().await;
    assert_eq!(fixture.provider.request_bodies().len(), 1);
    assert_eq!(
        tool_schema(&fixture.provider.request_bodies()[0]).as_deref(),
        Some("string")
    );
    let mut settlement = SettlementReceiptWait::capture(&runtime);
    fixture.gates[1].release();
    settlement.wait().await;
    fixture.close().await;
    mcp.close().await;
}

#[tokio::test]
async fn submission_clock_survives_unload_handoff_and_competing_attach_claims() {
    let mcp = McpFixture::start().await;
    let mut fixture = Fixture::with_session_count(None, 0).await;
    configure(&fixture, &[("late", &mcp)]);
    let session = create(&fixture).await;
    fixture.sessions.push(session.clone());
    fixture.manager.load(&session.id, None).await.unwrap();
    let id = session.active_conversation_id.clone();
    let probe = fixture.manager.probe(&id);
    probe.before_shutdown.arm();
    let unload = super::super::unload_task(&fixture, id.clone());
    probe.before_shutdown.entered().await;
    let received = crate::local_runtime::composition::SubmissionIntent::now();
    let latest_original_deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(15);
    let received = fixture
        .manager
        .accept_submission_intent(&session.id, &session.active_node, received)
        .unwrap();
    fixture
        .manager
        .record_submission_intent(received)
        .await
        .unwrap();
    let competing = fixture.load(0);
    tokio::time::pause();
    tokio::time::advance(std::time::Duration::from_secs(10)).await;
    tokio::task::yield_now().await;
    tokio::time::resume();
    mcp.control.gate.arm();
    probe.before_shutdown.release();
    bounded(mcp.control.gate.entered()).await;
    let deadline = {
        let registry = fixture.manager.registry.0.lock().unwrap();
        let super::super::super::Entry::Loading(flight) = &registry.entries[&id] else {
            panic!("the handoff retains one loading owner");
        };
        flight.startup.deadline().deadline().unwrap()
    };
    assert!(
        deadline <= latest_original_deadline,
        "handoff cannot restart the first submission budget"
    );
    let next = fixture
        .manager
        .accept_submission_intent(
            &session.id,
            &session.active_node,
            crate::local_runtime::composition::SubmissionIntent::now(),
        )
        .unwrap();
    fixture
        .manager
        .record_submission_intent(next)
        .await
        .unwrap();
    let second = fixture.load(0);
    {
        let registry = fixture.manager.registry.0.lock().unwrap();
        let super::super::super::Entry::Loading(flight) = &registry.entries[&id] else {
            panic!("loading");
        };
        assert_eq!(
            flight.startup.deadline().deadline(),
            Some(deadline),
            "another waiter cannot reset the clock"
        );
    }
    mcp.control.gate.release();
    bounded(unload).await.unwrap().unwrap();
    let competing = bounded(competing).await.unwrap().unwrap();
    let second = bounded(second).await.unwrap().unwrap();
    assert_eq!(competing.incarnation_id(), second.incarnation_id());
    assert_eq!(probe.compositions.load(Ordering::SeqCst), 2);
    assert_eq!(mcp.control.lists.load(Ordering::SeqCst), 2);
    fixture.close().await;
    mcp.close().await;
}

/// Both manager retirement paths cancel the real unarmed HTTP preparation;
/// its late server answer cannot publish a resident or start a provider request.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unload_and_drain_cancel_unarmed_loading_and_retire_bound_receipts() {
    bounded(async {
        for (drain, with_receipt) in [(false, false), (true, false), (false, true), (true, true)] {
            let mcp = McpFixture::start().await;
            let mut fixture = Fixture::with_session_count(None, 0).await;
            configure(&fixture, &[("late", &mcp)]);
            mcp.control.gate.arm();
            let session = create(&fixture).await;
            fixture.sessions.push(session.clone());
            let loading = fixture.load(0);
            mcp.control.gate.entered().await;
            assert!(!loading.is_finished());
            assert!(fixture.provider.request_bodies().is_empty());
            let probe = fixture.manager.probe(&session.active_conversation_id);
            let receipt = if with_receipt {
                probe.before_submission_registration.arm();
                let connection =
                    crate::app_server::connection::AppServerConnection::new(fixture.host.clone());
                super::super::protocol::initialize(&connection).await;
                let receipt =
                    connection.admitted_request_for_test(crate::app_server::protocol::Request {
                        jsonrpc: crate::app_server::protocol::JsonRpcVersion::V2,
                        id: crate::app_server::protocol::RequestId::Integer(731),
                        call: crate::app_server::protocol::Method::SessionSubmissionIntent {
                            session_id: session.id.clone(),
                            node_id: session.active_node.clone(),
                        },
                    });
                probe.before_submission_registration.entered().await;
                Some(receipt)
            } else {
                None
            };
            if drain {
                assert!(fixture.manager.drain_all_runtimes().await.is_empty());
            } else {
                fixture
                    .manager
                    .unload(&session.active_conversation_id)
                    .await
                    .unwrap();
            }
            assert!(matches!(loading.await.unwrap(), Err(
                crate::local_runtime::session_runtime_manager::RuntimeManagerError::LoadingCancelled
            )));
            assert!(fixture.manager.is_empty());
            if let Some(receipt) = receipt {
                probe.before_submission_registration.release();
                assert!(matches!(
                    receipt.await.produced(),
                    crate::app_server::protocol::Response::Failure(_)
                ));
            }
            assert!(
                fixture.manager.is_empty(),
                "an old bound receipt cannot resurrect cancelled initialization"
            );
            assert_eq!(probe.compositions.load(Ordering::SeqCst), 1);
            assert!(fixture.manager.configuration_runtime(&session.id).is_none());
            let mut completed = mcp.control.completed.subscribe();
            mcp.control.gate.release();
            completed
                .wait_for(|completed| *completed == 1)
                .await
                .unwrap();
            assert!(
                fixture.manager.is_empty(),
                "late live result cannot activate a retired load"
            );
            assert!(fixture.provider.request_bodies().is_empty());
            assert_eq!(mcp.control.lists.load(Ordering::SeqCst), 1);
            fixture.close().await;
            mcp.close().await;
        }
    })
    .await;
}

/// Accepted native receipts are registered even while their protocol futures
/// remain unpolled. A second receipt cannot replace the first absolute budget.
#[tokio::test]
async fn first_unpolled_submission_receipt_keeps_its_absolute_deadline() {
    use crate::app_server::connection::AppServerConnection;
    use crate::app_server::protocol::{
        JsonRpcVersion, Method, MethodResult, Request, RequestId, Response,
    };

    let mcp = McpFixture::start().await;
    let mut fixture = Fixture::with_session_count(None, 0).await;
    configure(&fixture, &[("late", &mcp)]);
    mcp.control.gate.arm();
    let session = create(&fixture).await;
    fixture.sessions.push(session.clone());
    let loading = fixture.load(0);
    bounded(mcp.control.gate.entered()).await;
    let connection = AppServerConnection::new(fixture.host.clone());
    super::super::protocol::initialize(&connection).await;
    tokio::time::pause();
    let accepted = tokio::time::Instant::now();
    let first = connection.admitted_request_for_test(Request {
        jsonrpc: JsonRpcVersion::V2,
        id: RequestId::Integer(701),
        call: Method::SessionSubmissionIntent {
            session_id: session.id.clone(),
            node_id: session.active_node.clone(),
        },
    });
    tokio::time::advance(std::time::Duration::from_secs(10)).await;
    let second = connection.admitted_request_for_test(Request {
        jsonrpc: JsonRpcVersion::V2,
        id: RequestId::Integer(702),
        call: Method::SessionSubmissionIntent {
            session_id: session.id.clone(),
            node_id: session.active_node.clone(),
        },
    });
    // Consume the later protocol waiter first, leaving the original unpolled.
    let later = second.await;
    assert!(matches!(later.produced(), Response::Success(success)
        if matches!(success.result, MethodResult::SubmissionIntentAccepted {})));
    let earlier = first.await;
    assert!(matches!(earlier.produced(), Response::Success(success)
        if matches!(success.result, MethodResult::SubmissionIntentAccepted {})));
    {
        let registry = fixture.manager.registry.0.lock().unwrap();
        let super::super::super::Entry::Loading(flight) =
            &registry.entries[&session.active_conversation_id]
        else {
            panic!("held discovery remains loading");
        };
        assert_eq!(
            flight.startup.deadline().deadline(),
            Some(
                accepted
                    .checked_add(std::time::Duration::from_secs(15))
                    .unwrap()
            )
        );
    }
    tokio::time::advance(std::time::Duration::from_secs(4)).await;
    tokio::task::yield_now().await;
    assert!(!loading.is_finished());
    assert!(fixture.provider.request_bodies().is_empty());
    tokio::time::advance(std::time::Duration::from_secs(1)).await;
    tokio::task::yield_now().await;
    tokio::time::resume();
    bounded(loading).await.unwrap().unwrap();
    let runtime = fixture.manager.configuration_runtime(&session.id).unwrap();
    let unavailable = || {
        runtime
            .runtime_resources()
            .capability_availability()
            .values()
            .any(|availability| {
                matches!(
                    availability,
                    crate::capabilities::CapabilitySourceState::Unavailable { .. }
                )
            })
    };
    assert!(unavailable(), "the first absolute deadline is terminal");
    let revision = runtime.runtime_resources().revision();
    let mut completed = mcp.control.completed.subscribe();
    mcp.control.gate.release();
    bounded(completed.wait_for(|completed| *completed == 1))
        .await
        .unwrap();
    assert!(unavailable());
    assert_eq!(runtime.runtime_resources().revision(), revision);
    assert!(fixture.provider.request_bodies().is_empty());
    fixture.close().await;
    mcp.close().await;
}

/// Exact-node receipts reach the live loading clock before a blocked catalog
/// lookup. An unrelated node cannot consume that preparation's budget.
#[tokio::test]
async fn accepted_exact_node_clock_precedes_blocked_catalog_lookup() {
    use crate::app_server::connection::AppServerConnection;
    use crate::app_server::protocol::{
        JsonRpcVersion, Method, MethodResult, Request, RequestId, Response,
    };

    let mcp = McpFixture::start().await;
    let mut fixture = Fixture::with_session_count(None, 0).await;
    configure(&fixture, &[("late", &mcp)]);
    mcp.control.gate.arm();
    let session = create(&fixture).await;
    fixture.sessions.push(session.clone());
    let loading = fixture.load(0);
    bounded(mcp.control.gate.entered()).await;
    let connection = AppServerConnection::new(fixture.host.clone());
    super::super::protocol::initialize(&connection).await;
    let catalog = fixture.manager.sessions.catalog.lock().await;
    let wrong_node = crate::local_runtime::session::SessionNodeId::new(
        "node_0199c989-03a0-7000-8000-000000000099",
    );
    let wrong = connection.admitted_request_for_test(Request {
        jsonrpc: JsonRpcVersion::V2,
        id: RequestId::Integer(711),
        call: Method::SessionSubmissionIntent {
            session_id: session.id.clone(),
            node_id: wrong_node,
        },
    });
    tokio::time::pause();
    tokio::time::advance(std::time::Duration::from_secs(60)).await;
    {
        let registry = fixture.manager.registry.0.lock().unwrap();
        let super::super::super::Entry::Loading(flight) =
            &registry.entries[&session.active_conversation_id]
        else {
            panic!("unarmed selected source remains loading");
        };
        assert_eq!(flight.startup.deadline().deadline(), None);
    }
    assert!(!loading.is_finished());
    tokio::time::resume();
    drop(catalog);
    assert!(matches!(
        bounded(wrong).await.produced(),
        Response::Failure(_)
    ));

    let catalog = fixture.manager.sessions.catalog.lock().await;
    let first = connection.admitted_request_for_test(Request {
        jsonrpc: JsonRpcVersion::V2,
        id: RequestId::Integer(712),
        call: Method::SessionSubmissionIntent {
            session_id: session.id.clone(),
            node_id: session.active_node.clone(),
        },
    });
    tokio::time::pause();
    tokio::time::advance(std::time::Duration::from_secs(20)).await;
    tokio::task::yield_now().await;
    {
        let registry = fixture.manager.registry.0.lock().unwrap();
        let super::super::super::Entry::Loading(flight) =
            &registry.entries[&session.active_conversation_id]
        else {
            panic!("post-preparation metadata remains blocked by the catalog owner");
        };
        assert!(flight.startup.deadline().has_expired());
    }
    assert!(fixture.provider.request_bodies().is_empty());
    tokio::time::resume();
    let mut completed = mcp.control.completed.subscribe();
    mcp.control.gate.release();
    bounded(completed.wait_for(|completed| *completed == 1))
        .await
        .unwrap();
    // The peer has produced its late result while the receipt's async lookup
    // is still blocked. Releasing metadata cannot turn that result into Ready.
    drop(catalog);
    assert!(
        matches!(bounded(first).await.produced(), Response::Success(success)
        if matches!(success.result, MethodResult::SubmissionIntentAccepted {}))
    );
    bounded(loading).await.unwrap().unwrap();
    let runtime = fixture.manager.configuration_runtime(&session.id).unwrap();
    assert!(
        runtime
            .runtime_resources()
            .capability_availability()
            .values()
            .any(|availability| matches!(
                availability,
                crate::capabilities::CapabilitySourceState::Unavailable { .. }
            ))
    );
    assert!(fixture.provider.request_bodies().is_empty());
    assert!(
        fixture
            .manager
            .registry
            .0
            .lock()
            .unwrap()
            .submission_receipts
            .is_empty()
    );
    fixture.close().await;
    mcp.close().await;
}

/// A cold attach can overtake the asynchronous receipt lookup, but consumes
/// the accepted exact-node clock before it starts any source preparation.
#[tokio::test]
async fn cold_attach_consumes_an_accepted_receipt_before_starting_mcp() {
    use crate::app_server::connection::AppServerConnection;
    use crate::app_server::protocol::{JsonRpcVersion, Method, Request, RequestId, Response};

    let mcp = McpFixture::start().await;
    let mut fixture = Fixture::with_session_count(None, 0).await;
    configure(&fixture, &[("late", &mcp)]);
    mcp.control.gate.arm();
    let session = create(&fixture).await;
    fixture.sessions.push(session.clone());
    let probe = fixture.manager.probe(&session.active_conversation_id);
    probe.before_submission_registration.arm();
    let connection = AppServerConnection::new(fixture.host.clone());
    super::super::protocol::initialize(&connection).await;
    tokio::time::pause();
    let accepted = tokio::time::Instant::now();
    let receipt = connection.admitted_request_for_test(Request {
        jsonrpc: JsonRpcVersion::V2,
        id: RequestId::Integer(721),
        call: Method::SessionSubmissionIntent {
            session_id: session.id.clone(),
            node_id: session.active_node.clone(),
        },
    });
    tokio::time::resume();
    bounded(probe.before_submission_registration.entered()).await;
    let loading = fixture.load(0);
    bounded(mcp.control.gate.entered()).await;
    {
        let registry = fixture.manager.registry.0.lock().unwrap();
        let super::super::super::Entry::Loading(flight) =
            &registry.entries[&session.active_conversation_id]
        else {
            panic!("held MCP preparation");
        };
        assert_eq!(
            flight.startup.deadline().deadline(),
            Some(
                accepted
                    .checked_add(std::time::Duration::from_secs(15))
                    .unwrap()
            )
        );
    }
    probe.before_submission_registration.release();
    assert!(matches!(
        bounded(receipt).await.produced(),
        Response::Success(_)
    ));
    mcp.control.gate.release();
    bounded(loading).await.unwrap().unwrap();
    assert_eq!(mcp.control.lists.load(Ordering::SeqCst), 1);
    assert!(fixture.provider.request_bodies().is_empty());
    assert!(
        fixture
            .manager
            .registry
            .0
            .lock()
            .unwrap()
            .submission_receipts
            .is_empty()
    );
    fixture.close().await;
    mcp.close().await;
}

/// Host drain closes native initialization before the first inventory. Accepted
/// metadata jobs cannot create a cold load or resurrect a cancelled loading owner.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn host_drain_fences_accepted_cold_and_loading_submission_receipts() {
    bounded(async {
        for already_loading in [false, true] {
            let mcp = McpFixture::start().await;
            let mut fixture = Fixture::with_session_count(None, 0).await;
            configure(&fixture, &[("late", &mcp)]);
            mcp.control.gate.arm();
            let session = create(&fixture).await;
            fixture.sessions.push(session.clone());
            let loading = if already_loading {
                let loading = fixture.load(0);
                mcp.control.gate.entered().await;
                Some(loading)
            } else { None };
            let probe = fixture.manager.probe(&session.active_conversation_id);
            probe.before_submission_registration.arm();
            let connection = crate::app_server::connection::AppServerConnection::new(fixture.host.clone());
            super::super::protocol::initialize(&connection).await;
            let receipt = connection.admitted_request_for_test(crate::app_server::protocol::Request {
                jsonrpc:crate::app_server::protocol::JsonRpcVersion::V2,
                id:crate::app_server::protocol::RequestId::Integer(741),
                call:crate::app_server::protocol::Method::SessionSubmissionIntent {
                    session_id:session.id.clone(), node_id:session.active_node.clone(),
                },
            });
            probe.before_submission_registration.entered().await;
            let compositions = probe.compositions.load(Ordering::SeqCst);
            let second_owner = fixture.host.admit_request(
                crate::app_server::RequestLane::Control, std::convert::identity,
            ).unwrap();
            fixture.host.begin_drain();
            let host = fixture.host.clone();
            let draining = tokio::spawn(async move {host.drain().await});
            if let Some(loading) = loading {
                assert!(matches!(loading.await.unwrap(), Err(
                    crate::local_runtime::session_runtime_manager::RuntimeManagerError::LoadingCancelled
                )));
            }
            assert!(fixture.manager.is_empty());
            probe.before_submission_registration.release();
            assert!(matches!(receipt.await.produced(), crate::app_server::protocol::Response::Failure(_)));
            assert!(fixture.manager.is_empty());
            assert_eq!(probe.compositions.load(Ordering::SeqCst), compositions);
            assert!(fixture.provider.request_bodies().is_empty());
            assert!(!draining.is_finished(), "the second native request still prevents the final inventory");
            if already_loading {
                let mut completed = mcp.control.completed.subscribe();
                mcp.control.gate.release();
                completed.wait_for(|completed| *completed == 1).await.unwrap();
            }
            assert!(fixture.provider.request_bodies().is_empty());
            drop(second_owner);
            assert!(draining.await.unwrap().is_empty());
            assert!(fixture.manager.is_empty());
            assert!(fixture.manager.registry.0.lock().unwrap().submission_receipts.is_empty());
            fixture.close().await;
            mcp.close().await;
        }
    }).await;
}

/// A receipt belongs to the initialization it first joined. Failing that owner
/// cannot let the receipt follow a retry, outlive its unload, and create a third.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn failed_initialization_receipt_cannot_follow_retry_and_resurrect_after_unload() {
    bounded(async {
        let mcp = McpFixture::named("not_the_selected_tool").await;
        let mut fixture = Fixture::with_session_count(None, 0).await;
        configure(&fixture, &[("late", &mcp)]);
        let config_path = fixture.workspaces[0].join("rustx.toml");
        let mut config: toml::Value = std::fs::read_to_string(&config_path)
            .unwrap()
            .parse()
            .unwrap();
        config["agent"]["tools"]["sources"]["late"] = toml::Value::Array(vec![TOOL.into()]);
        // The test's outer liveness bound, rather than elapsed startup policy,
        // determines whether this controlled wrong-name result completes.
        config.as_table_mut().unwrap().insert(
            "mcp_startup_policy".into(),
            toml::toml! { wait_timeout_ms = 60_000 }.into(),
        );
        std::fs::write(config_path, toml::to_string_pretty(&config).unwrap()).unwrap();
        let session = create(&fixture).await;
        fixture.sessions.push(session.clone());
        let probe = fixture.manager.probe(&session.active_conversation_id);
        probe.before_submission_registration.arm();
        let connection =
            crate::app_server::connection::AppServerConnection::new(fixture.host.clone());
        super::super::protocol::initialize(&connection).await;
        let receipt = connection.admitted_request_for_test(crate::app_server::protocol::Request {
            jsonrpc: crate::app_server::protocol::JsonRpcVersion::V2,
            id: crate::app_server::protocol::RequestId::Integer(751),
            call: crate::app_server::protocol::Method::SessionSubmissionIntent {
                session_id: session.id.clone(),
                node_id: session.active_node.clone(),
            },
        });
        probe.before_submission_registration.entered().await;
        assert!(
            fixture.load(0).await.unwrap().is_err(),
            "complete live absence rejects the exact selection"
        );
        assert!(matches!(
            fixture
                .manager
                .registry
                .0
                .lock()
                .unwrap()
                .entries
                .get(&session.active_conversation_id),
            Some(Entry::FailedSubmission(_))
        ));
        fixture
            .manager
            .unload(&session.active_conversation_id)
            .await
            .unwrap();
        assert!(fixture.manager.is_empty());
        *mcp.control.name.lock().unwrap() = TOOL;
        fixture.load(0).await.unwrap().unwrap();
        fixture
            .manager
            .unload(&session.active_conversation_id)
            .await
            .unwrap();
        assert!(fixture.manager.is_empty());
        probe.before_submission_registration.release();
        assert!(matches!(
            receipt.await.produced(),
            crate::app_server::protocol::Response::Failure(_)
        ));
        assert!(fixture.manager.is_empty());
        assert_eq!(probe.compositions.load(Ordering::SeqCst), 2);
        assert_eq!(mcp.control.lists.load(Ordering::SeqCst), 2);
        assert!(fixture.provider.request_bodies().is_empty());
        assert!(
            fixture
                .manager
                .registry
                .0
                .lock()
                .unwrap()
                .submission_receipts
                .is_empty()
        );
        fixture.close().await;
        mcp.close().await;
    })
    .await;
}

/// Recovery recognizes accepted work before MCP preparation, including an
/// adopted answer obligation whose Pending Inbox is already empty.
#[tokio::test]
#[allow(clippy::too_many_lines)] // both durable forms share the real clock/admission path
async fn durable_recovery_receipt_precedes_later_send_and_bounds_initial_mcp() {
    use crate::app_server::connection::AppServerConnection;
    use crate::app_server::protocol::{Method, MethodResult};
    use crate::durable::inbox::InboxStore;

    for adopted in [false, true] {
        let late = McpFixture::start().await;
        let mut fixture = Fixture::with_session_count(None, 0).await;
        configure(&fixture, &[("late", &late)]);
        late.control.gate.arm();
        let session = create(&fixture).await;
        fixture.sessions.push(session.clone());
        {
            let access = fixture
                .manager
                .sessions
                .acquire_session(&session.id, None)
                .await
                .unwrap();
            let store = crate::durable::SqliteConversationStore::open(
                session.active_conversation_id.clone(),
                &access.database_path,
            )
            .unwrap();
            let accepted = store
                .accept_inbound(crate::durable::InboundDraft {
                    message_id: None,
                    source: crate::message::types::UserSource::Human,
                    kind: crate::message::types::InboundKind::Message,
                    content: input("request-B: accepted work awaiting native recovery"),
                    timestamp: chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap(),
                    correlation: None,
                })
                .unwrap();
            if adopted {
                store.adopt_pending_batch(accepted.sequence, None).unwrap();
                assert!(store.load_pending().unwrap().is_empty());
            }
        }
        let connection = AppServerConnection::new(fixture.host.clone());
        super::super::protocol::initialize(&connection).await;
        let probe = fixture.manager.probe(&session.active_conversation_id);
        probe.before_recovery_inspection.arm();
        tokio::time::pause();
        let received = tokio::time::Instant::now();
        let loading = fixture.load(0);
        probe.before_recovery_inspection.entered().await;
        tokio::time::advance(std::time::Duration::from_secs(10)).await;
        assert!(matches!(
            super::super::protocol::call(
                &connection,
                790,
                Method::SessionSubmissionIntent {
                    session_id: session.id.clone(),
                    node_id: session.active_node.clone(),
                }
            )
            .await,
            MethodResult::SubmissionIntentAccepted {}
        ));
        let startup = {
            let registry = fixture.manager.registry.0.lock().unwrap();
            let super::super::super::Entry::Loading(flight) =
                &registry.entries[&session.active_conversation_id]
            else {
                panic!("recovery inspection remains held");
            };
            flight.startup.clone()
        };
        assert_eq!(
            startup.deadline().deadline(),
            received.checked_add(std::time::Duration::from_secs(25))
        );
        // Keep simulated time frozen while loopback I/O progresses. The real
        // time bound checks transport liveness without consuming the MCP budget.
        probe.before_recovery_inspection.release();
        let transport_started = std::time::Instant::now();
        while !*late.control.gate.entered.borrow() {
            assert!(transport_started.elapsed() < std::time::Duration::from_secs(15));
            tokio::task::yield_now().await;
        }
        let deadline = received
            .checked_add(std::time::Duration::from_secs(15))
            .unwrap();
        assert_eq!(startup.deadline().deadline(), Some(deadline));
        assert!(fixture.provider.request_bodies().is_empty());
        let remaining = deadline
            .checked_duration_since(tokio::time::Instant::now())
            .unwrap();
        tokio::time::advance(
            remaining
                .checked_sub(std::time::Duration::from_millis(1))
                .unwrap(),
        )
        .await;
        tokio::task::yield_now().await;
        assert!(!loading.is_finished());
        assert!(fixture.provider.request_bodies().is_empty());
        tokio::time::advance(std::time::Duration::from_millis(1)).await;
        tokio::time::resume();
        bounded(loading).await.unwrap().unwrap();
        let runtime = fixture.manager.configuration_runtime(&session.id).unwrap();
        bounded(fixture.gates[1].wait_entered()).await;
        let requests = fixture.provider.request_bodies();
        assert_eq!(requests.len(), 1);
        let request: serde_json::Value = serde_json::from_str(&requests[0]).unwrap();
        let tools = request["tools"].as_array().unwrap();
        assert!(tools.iter().any(|tool| tool["function"]["name"] == "read"));
        assert!(tools.iter().all(|tool| tool["function"]["name"] != TOOL));
        assert!(
            runtime
                .runtime_resources()
                .capability_availability()
                .values()
                .any(|availability| matches!(
                    availability,
                    crate::capabilities::CapabilitySourceState::Unavailable { .. }
                ))
        );
        let revision = runtime.runtime_resources().revision();
        let mut completed = late.control.completed.subscribe();
        late.control.gate.release();
        bounded(completed.wait_for(|completed| *completed == 1))
            .await
            .unwrap();
        assert_eq!(
            runtime.runtime_resources().revision(),
            revision,
            "a late discovery cannot revive the expired batch"
        );
        let mut settlement = SettlementReceiptWait::capture(&runtime);
        fixture.gates[1].release();
        bounded(settlement.wait()).await;
        assert_eq!(fixture.provider.request_bodies().len(), 1);
        fixture.close().await;
        late.close().await;
    }
}

/// A runtime fact does not wait for an independent background ownership
/// commit: that execution retains its frozen executor and physical leases.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(clippy::too_many_lines)] // Real ownership, live discovery and publication share one ordering proof.
async fn live_catalog_publication_never_parks_on_background_ownership() {
    bounded(async {
        struct ReleaseCommit(Arc<crate::tools::background::test_sync::CommitBoundaryHook>);
        impl Drop for ReleaseCommit {
            fn drop(&mut self) {
                self.0.proceed();
            }
        }
        let mcp = McpFixture::start().await;
        let mut fixture = Fixture::with_session_count(None, 0).await;
        configure(&fixture, &[("late", &mcp)]);
        let session = create(&fixture).await;
        fixture.sessions.push(session.clone());
        fixture.manager.load(&session.id, None).await.unwrap();
        let runtime = fixture.manager.configuration_runtime(&session.id).unwrap();
        let mut publications = runtime.runtime_catalog_publications();
        let before = *publications.borrow();
        let background = runtime.tool_runtime().background().clone();
        let hook = Arc::new(crate::tools::background::test_sync::CommitBoundaryHook::default());
        background.install_commit_boundary_hook(hook.clone());
        let _release_commit = ReleaseCommit(hook.clone());
        let (executor, mut started, release) = super::adoption::GatedBackgroundExecutor::new();
        let executor: Arc<dyn crate::tools::executor::ToolExecutor> = Arc::new(executor);
        let invocation = crate::tools::types::ToolInvocation {
            id: crate::tools::types::ToolInvocationId::Agent {
                call_id: crate::runtime::identity::ToolCallId::new("catalog-background-owner"),
            },
            tool_id: crate::runtime::identity::ToolId::new("tool-bash"),
            tool_name: "bash".into(),
            mode: crate::tools::types::ToolInvocationMode::Background,
            arguments: serde_json::json!({}),
        };
        let prepared = background
            .prepare_dispatch(
                &invocation,
                &executor,
                crate::tools::environment::ToolEnvironment::new(),
            )
            .unwrap();
        let committing = background.clone();
        let commit = tokio::task::spawn_blocking(move || {
            committing.commit_dispatch(prepared, &crate::runtime::CancellationSignal::new())
        });
        let entered = hook.clone();
        tokio::task::spawn_blocking(move || entered.wait_entered())
            .await
            .unwrap();
        *mcp.control.schema_kind.lock().unwrap() = "integer";
        runtime
            .capability()
            .mcp_invalidation()
            .lock()
            .advance(&McpServerId::new("late"));
        assert!(matches!(
            execute_current_tool(&runtime, serde_json::json!({"id":"old"}))
                .await
                .status,
            crate::tools::types::ToolExecutionStatus::Failed { .. }
        ));
        wait_runtime_catalog(&runtime, &mut publications, before).await;
        assert!(
            !commit.is_finished(),
            "the background ownership mutex remains parked"
        );
        assert!(runtime.configuration_cut().admitted_attempt.is_none());
        assert!(fixture.provider.request_bodies().is_empty());
        hook.proceed();
        let crate::tools::background::BackgroundDispatchOutcome::Accepted { execution_id, .. } =
            commit.await.unwrap().unwrap()
        else {
            panic!("background ownership commits once released");
        };
        started.wait_for(|started| *started).await.unwrap();
        release.send_replace(true);
        background.wait_until_settled(&execution_id).await;
        assert_eq!(mcp.control.lists.load(Ordering::SeqCst), 2);
        assert_eq!(mcp.control.calls.load(Ordering::SeqCst), 0);
        assert!(fixture.provider.request_bodies().is_empty());
        fixture.close().await;
        mcp.close().await;
    })
    .await;
}

/// The catalog wins the admission cut even when its native lifecycle grant is
/// temporarily owned by another commit. Busy preserves the existing input kick.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(clippy::too_many_lines)] // Both native commit and admission owners are explicitly gated.
async fn live_catalog_busy_frontier_defers_existing_admission_without_parking_controls() {
    bounded(async {
        use crate::runtime::conversation_runtime::Gate;
        let mcp = McpFixture::start().await;
        let mut fixture = Fixture::with_session_count(None, 0).await;
        configure(&fixture, &[("late", &mcp)]);
        let session = create(&fixture).await;
        fixture.sessions.push(session.clone());
        fixture.manager.load(&session.id, None).await.unwrap();
        let runtime = fixture.manager.configuration_runtime(&session.id).unwrap();
        let mut publications = runtime.runtime_catalog_publications();
        let before = *publications.borrow();
        let admission = Arc::new(Gate::default());
        let release_admission = admission.arm_scoped();
        runtime.install_admission_gate(admission.clone());
        let submitting = runtime.clone();
        let submit = tokio::task::spawn_blocking(move || {
            submitting.submit_inbound(input("request-B: pending before catalog publication"))
        });
        super::super::gate_entered(&admission).await;
        let frontier = Arc::new(Gate::default());
        let release_frontier = frontier.arm_scoped();
        let lifecycle = runtime.title_lifecycle();
        let holding = frontier.clone();
        let held =
            tokio::task::spawn_blocking(move || lifecycle.with_running_commit(|| holding.enter()));
        super::super::gate_entered(&frontier).await;
        *mcp.control.schema_kind.lock().unwrap() = "integer";
        runtime
            .capability()
            .mcp_invalidation()
            .lock()
            .advance(&McpServerId::new("late"));
        assert!(matches!(
            execute_current_tool(&runtime, serde_json::json!({"id":"old"}))
                .await
                .status,
            crate::tools::types::ToolExecutionStatus::Failed { .. }
        ));
        drop(release_admission);
        runtime.runtime_catalog_blocked().await;
        assert_eq!(*publications.borrow(), before);
        assert!(runtime.has_pending_runtime_catalog());
        // A second distinct successful live catalog supersedes the retained
        // candidate while its lifecycle grant is still unavailable.
        *mcp.control.schema_kind.lock().unwrap() = "number";
        runtime
            .capability()
            .mcp_invalidation()
            .lock()
            .advance(&McpServerId::new("late"));
        assert!(matches!(
            execute_current_tool(&runtime, serde_json::json!({"id":"old"}))
                .await
                .status,
            crate::tools::types::ToolExecutionStatus::Failed { .. }
        ));
        submit.await.unwrap().unwrap();
        // This native control read completes while the frontier is still held.
        assert!(runtime.configuration_cut().admitted_attempt.is_none());
        assert!(fixture.provider.request_bodies().is_empty());
        assert_eq!(mcp.control.lists.load(Ordering::SeqCst), 3);
        drop(release_frontier);
        held.await.unwrap().unwrap();
        wait_runtime_catalog(&runtime, &mut publications, before).await;
        fixture.gates[1].wait_entered().await;
        assert_eq!(fixture.provider.request_bodies().len(), 1);
        assert_eq!(
            tool_schema(&fixture.provider.request_bodies()[0]).as_deref(),
            Some("number")
        );
        assert!(!runtime.has_pending_runtime_catalog());
        let mut settlement = SettlementReceiptWait::capture(&runtime);
        fixture.gates[1].release();
        settlement.wait().await;
        assert_eq!(mcp.control.lists.load(Ordering::SeqCst), 3);
        assert_eq!(mcp.control.calls.load(Ordering::SeqCst), 0);
        fixture.close().await;
        mcp.close().await;
    })
    .await;
}

/// A successful explicit refresh held behind a whole Attempt is still owned
/// by native drain, which settles both current and unpublished generations.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shutdown_retires_successful_refresh_queued_behind_an_active_attempt() {
    bounded(async {
        let mcp = McpFixture::start().await;
        let mut fixture = Fixture::with_session_count(None, 0).await;
        configure(&fixture, &[("late", &mcp)]);
        let session = create(&fixture).await;
        fixture.sessions.push(session.clone());
        fixture.manager.load(&session.id, None).await.unwrap();
        let runtime = fixture.manager.configuration_runtime(&session.id).unwrap();
        runtime
            .submit_inbound(input("request-B: active while refresh completes"))
            .unwrap();
        fixture.gates[1].wait_entered().await;
        let admitted = runtime.configuration_cut().admitted_attempt.unwrap();
        let revision = runtime.runtime_resources().revision();
        runtime.refresh_runtime_catalogs().await.unwrap();
        assert!(runtime.has_pending_runtime_catalog());
        assert_eq!(runtime.runtime_resources().revision(), revision);
        assert_eq!(
            runtime.configuration_cut().admitted_attempt.as_ref(),
            Some(&admitted)
        );
        assert_eq!(
            mcp.control.lists.load(Ordering::SeqCst),
            2,
            "startup and one distinct refreshed generation completed"
        );
        let drained = Arc::new(tokio::sync::Notify::new());
        runtime.install_drain_signals(Arc::new(tokio::sync::Notify::new()), drained.clone());
        let shutting = runtime.clone();
        let shutdown = tokio::spawn(async move { shutting.shutdown().await });
        drained.notified().await;
        fixture.gates[1].release();
        shutdown.await.unwrap().unwrap();
        assert!(!runtime.has_pending_runtime_catalog());
        assert_eq!(runtime.capability().pending_mcp_retirements(), 0);
        assert_eq!(runtime.runtime_resources().revision(), revision);
        assert_eq!(fixture.provider.request_bodies().len(), 1);
        fixture.close().await;
        mcp.close().await;
    })
    .await;
}

/// A completed refresh belongs to the source authority it captured, even when
/// its native completion is delayed until after an authored source adoption.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn superseded_refresh_completion_cannot_publish_into_authored_source_binding() {
    bounded(async {
        use crate::runtime::conversation_runtime::Gate;
        for fails in [false, true] {
            let original = McpFixture::start().await;
            let replacement = McpFixture::named("replacement_echo").await;
            let mut fixture = Fixture::with_session_count(None, 0).await;
            configure(&fixture, &[("late", &original)]);
            let session = create(&fixture).await;
            fixture.sessions.push(session.clone());
            fixture.manager.load(&session.id, None).await.unwrap();
            let runtime = fixture.manager.configuration_runtime(&session.id).unwrap();
            let completed = Arc::new(Gate::default());
            let release = completed.arm_scoped();
            runtime.install_runtime_refresh_completed_gate(completed.clone());
            original.control.fail.store(fails, Ordering::SeqCst);
            let refreshing = runtime.clone();
            let refresh = tokio::spawn(async move { refreshing.refresh_runtime_catalogs().await });
            super::super::gate_entered(&completed).await;
            std::fs::write(
                fixture.workspaces[0].join(".agents/mcp.toml"),
                format!("[mcp_servers.late]\nurl = {:?}\n", replacement.endpoint),
            )
            .unwrap();
            write(
                &fixture,
                0,
                ConfigMutation::Instructions {
                    authored: Some("adopted replacement instructions".into()),
                },
            )
            .await;
            let candidate = settled(&fixture, 0).await.candidate.unwrap();
            runtime.wait_for_configuration_admissions().await;
            fixture
                .manager
                .adopt_configuration(&session.id, &candidate.identity, candidate.expected_binding)
                .unwrap();
            let adopted = runtime.configuration_cut();
            assert!(
                adopted
                    .resources
                    .capability()
                    .tool_registry()
                    .definitions()
                    .iter()
                    .any(|tool| tool.name == "replacement_echo")
            );
            drop(release);
            assert!(refresh.await.unwrap().is_err());
            let current = runtime.configuration_cut();
            assert_eq!(current.binding_revision, adopted.binding_revision);
            assert_eq!(current.resources.revision(), adopted.resources.revision());
            assert_eq!(
                current.resources.capability().revision(),
                adopted.resources.capability().revision()
            );
            assert!(
                current
                    .resources
                    .inspection()
                    .resource_diagnostics
                    .iter()
                    .all(|diagnostic| diagnostic.field != "runtime.catalog")
            );
            assert!(fixture.provider.request_bodies().is_empty());
            assert_eq!(original.control.lists.load(Ordering::SeqCst), 2);
            assert_eq!(replacement.control.lists.load(Ordering::SeqCst), 1);
            fixture.close().await;
            original.close().await;
            replacement.close().await;
        }
    })
    .await;
}

/// An already eligible success may still publish after a later explicit
/// refresh fails. Publishing it cannot erase that newer truthful failure.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pending_success_publishes_without_erasing_newer_explicit_refresh_failure() {
    bounded(async {
        let mcp = McpFixture::start().await;
        let mut fixture = Fixture::with_session_count(None, 0).await;
        configure(&fixture, &[("late", &mcp)]);
        let session = create(&fixture).await;
        fixture.sessions.push(session.clone());
        fixture.manager.load(&session.id, None).await.unwrap();
        let runtime = fixture.manager.configuration_runtime(&session.id).unwrap();
        runtime
            .submit_inbound(input("request-B: hold the publication cut"))
            .unwrap();
        fixture.gates[1].wait_entered().await;
        let mut settlement = SettlementReceiptWait::capture(&runtime);
        let mut publications = runtime.runtime_catalog_publications();
        let before = *publications.borrow();
        runtime.refresh_runtime_catalogs().await.unwrap();
        assert!(runtime.has_pending_runtime_catalog());
        mcp.control.fail.store(true, Ordering::SeqCst);
        assert!(runtime.refresh_runtime_catalogs().await.is_err());
        let failure = catalog_failure(&runtime).unwrap();
        assert!(failure.starts_with("last explicit MCP refresh failed:"));
        assert!(runtime.has_pending_runtime_catalog());
        fixture.gates[1].release();
        settlement.wait().await;
        wait_runtime_catalog(&runtime, &mut publications, before).await;
        assert!(!runtime.has_pending_runtime_catalog());
        assert_eq!(catalog_failure(&runtime).as_ref(), Some(&failure));
        assert_eq!(mcp.control.lists.load(Ordering::SeqCst), 3);
        assert_eq!(fixture.provider.request_bodies().len(), 1);
        write(
            &fixture,
            0,
            ConfigMutation::Instructions {
                authored: Some("context after retained success".into()),
            },
        )
        .await;
        let candidate = settled(&fixture, 0).await.candidate.unwrap();
        runtime.wait_for_configuration_admissions().await;
        fixture
            .manager
            .adopt_configuration(&session.id, &candidate.identity, candidate.expected_binding)
            .unwrap();
        assert_eq!(catalog_failure(&runtime).as_ref(), Some(&failure));
        mcp.control.fail.store(false, Ordering::SeqCst);
        let before = *publications.borrow();
        runtime.refresh_runtime_catalogs().await.unwrap();
        wait_runtime_catalog(&runtime, &mut publications, before).await;
        assert!(catalog_failure(&runtime).is_none());
        assert_eq!(mcp.control.lists.load(Ordering::SeqCst), 4);
        assert_eq!(fixture.provider.request_bodies().len(), 1);
        fixture.close().await;
        mcp.close().await;
    })
    .await;
}

async fn reconcile_mcp_catalog(
    connection: &crate::app_server::connection::AppServerConnection,
    target: &crate::local_runtime::configuration::settings::SourceTarget,
    request_id: i64,
) {
    use crate::app_server::protocol::{Method, MethodResult};
    assert!(matches!(
        super::super::protocol::call(
            connection,
            request_id,
            Method::ConfigurationReconcile {
                target: target.clone(),
            },
        )
        .await,
        MethodResult::ConfigurationReceipt { .. }
    ));
}

/// All three refreshes enter through the public App Server reconcile route.
/// A newer explicit failure remains retryable after an older pending success
/// has installed a Ready/current-epoch catalog.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn public_reconcile_retries_retained_failure_after_older_success_publication() {
    bounded(async {
        use crate::app_server::connection::AppServerConnection;
        use crate::local_runtime::configuration::settings::SourceTarget;
        use crate::runtime::conversation_runtime::Gate;

        let mcp = McpFixture::start().await;
        let mut fixture = Fixture::with_session_count(None, 0).await;
        configure(&fixture, &[("late", &mcp)]);
        let session = create(&fixture).await;
        fixture.sessions.push(session.clone());
        fixture.manager.load(&session.id, None).await.unwrap();
        let runtime = fixture.manager.configuration_runtime(&session.id).unwrap();
        let connection = AppServerConnection::new(fixture.host.clone());
        super::super::protocol::initialize(&connection).await;
        let target = SourceTarget::Workspace {
            directory: fixture.workspaces[0].clone(),
        };
        runtime
            .submit_inbound(input("request-B: hold the public refresh publication cut"))
            .unwrap();
        fixture.gates[1].wait_entered().await;
        let admitted = runtime.configuration_cut().admitted_attempt.unwrap();
        let mut settlement = SettlementReceiptWait::capture(&runtime);
        let mut publications = runtime.runtime_catalog_publications();
        let before = *publications.borrow();

        // The selected source's ordinary invalidation makes public R1 and R2
        // eligible while the old adopted epoch remains frozen by the Attempt.
        runtime
            .capability()
            .mcp_invalidation()
            .lock()
            .advance(&McpServerId::new("late"));
        let completed = Arc::new(Gate::default());
        let release = completed.arm_scoped();
        runtime.install_runtime_refresh_completed_gate(completed.clone());
        reconcile_mcp_catalog(&connection, &target, 100).await;
        super::super::gate_entered(&completed).await;
        drop(release);
        while !runtime.has_pending_runtime_catalog() {
            tokio::task::yield_now().await;
        }
        assert_eq!(mcp.control.lists.load(Ordering::SeqCst), 2);

        mcp.control.fail.store(true, Ordering::SeqCst);
        reconcile_mcp_catalog(&connection, &target, 101).await;
        let failure = loop {
            if let Some(failure) = catalog_failure(&runtime) {
                break failure;
            }
            tokio::task::yield_now().await;
        };
        assert!(failure.starts_with("last explicit MCP refresh failed:"));
        assert!(runtime.has_pending_runtime_catalog());
        assert_eq!(mcp.control.lists.load(Ordering::SeqCst), 3);
        assert_eq!(runtime.configuration_cut().admitted_attempt, Some(admitted));
        fixture.gates[1].release();
        settlement.wait().await;
        wait_runtime_catalog(&runtime, &mut publications, before).await;
        assert!(!runtime.has_pending_runtime_catalog());
        assert_eq!(catalog_failure(&runtime).as_ref(), Some(&failure));
        assert!(!runtime.capability().needs_mcp_refresh());
        assert!(
            runtime
                .runtime_resources()
                .capability_availability()
                .iter()
                .any(|(source, state)| matches!(source,
                crate::capabilities::ToolSourceId::Mcp(id) if id == &McpServerId::new("late"))
                    && matches!(state, crate::capabilities::CapabilitySourceState::Ready))
        );
        assert_eq!(fixture.provider.request_bodies().len(), 1);

        mcp.control.fail.store(false, Ordering::SeqCst);
        mcp.control.gate.arm();
        let before = *publications.borrow();
        reconcile_mcp_catalog(&connection, &target, 102).await;
        let reached = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            mcp.control.gate.entered(),
        )
        .await;
        if reached.is_err() {
            let listings = mcp.control.lists.load(Ordering::SeqCst);
            mcp.control.gate.release();
            fixture.close().await;
            mcp.close().await;
            panic!("public R3 must retry the retained current failure; listings={listings}");
        }
        assert_eq!(mcp.control.lists.load(Ordering::SeqCst), 4);
        assert_eq!(catalog_failure(&runtime).as_ref(), Some(&failure));
        mcp.control.gate.release();
        wait_runtime_catalog(&runtime, &mut publications, before).await;
        assert!(catalog_failure(&runtime).is_none());
        assert_eq!(mcp.control.lists.load(Ordering::SeqCst), 4);
        assert_eq!(mcp.control.calls.load(Ordering::SeqCst), 0);
        assert_eq!(fixture.provider.request_bodies().len(), 1);
        fixture.close().await;
        mcp.close().await;
    })
    .await;
}

fn catalog_failure(runtime: &crate::runtime::ConversationRuntime) -> Option<String> {
    runtime
        .runtime_resources()
        .inspection()
        .resource_diagnostics
        .iter()
        .find(|diagnostic| diagnostic.field == "runtime.catalog")
        .map(|diagnostic| diagnostic.reason.clone())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sibling_live_catalog_success_preserves_failed_source_refresh_diagnostic() {
    bounded(async {
        let failed = McpFixture::named("other_echo").await;
        let healthy = McpFixture::start().await;
        let mut fixture = Fixture::with_session_count(None, 0).await;
        configure(&fixture, &[("failed", &failed), ("healthy", &healthy)]);
        let session = create(&fixture).await;
        fixture.sessions.push(session.clone());
        fixture.manager.load(&session.id, None).await.unwrap();
        let runtime = fixture.manager.configuration_runtime(&session.id).unwrap();
        failed.control.fail.store(true, Ordering::SeqCst);
        assert!(runtime.refresh_runtime_catalogs().await.is_err());
        let failure = catalog_failure(&runtime).unwrap();
        *healthy.control.schema_kind.lock().unwrap() = "integer";
        let mut publications = runtime.runtime_catalog_publications();
        let before = *publications.borrow();
        runtime
            .capability()
            .mcp_invalidation()
            .lock()
            .advance(&McpServerId::new("healthy"));
        assert!(matches!(
            execute_current_tool(&runtime, serde_json::json!({"id":"old"}))
                .await
                .status,
            crate::tools::types::ToolExecutionStatus::Failed { .. }
        ));
        wait_runtime_catalog(&runtime, &mut publications, before).await;
        assert_eq!(catalog_failure(&runtime).as_ref(), Some(&failure));
        assert_eq!(failed.control.lists.load(Ordering::SeqCst), 2);
        assert_eq!(healthy.control.lists.load(Ordering::SeqCst), 3);
        assert!(fixture.provider.request_bodies().is_empty());
        failed.control.fail.store(false, Ordering::SeqCst);
        let before = *publications.borrow();
        runtime.refresh_runtime_catalogs().await.unwrap();
        wait_runtime_catalog(&runtime, &mut publications, before).await;
        assert!(catalog_failure(&runtime).is_none());
        assert_eq!(failed.control.lists.load(Ordering::SeqCst), 3);
        assert_eq!(healthy.control.lists.load(Ordering::SeqCst), 4);
        assert!(fixture.provider.request_bodies().is_empty());
        fixture.close().await;
        failed.close().await;
        healthy.close().await;
    })
    .await;
}

/// Recovery of unchanged unavailable R survives an unrelated Todo adoption
/// which replaces healthy S, and publication retains the new native tools.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn completed_recovery_rebases_over_todo_adoption_and_sibling_replacement() {
    bounded(async {
        use crate::runtime::conversation_runtime::Gate;
        let recovering = McpFixture::named("recovered_echo").await;
        recovering.control.fail.store(true, Ordering::SeqCst);
        let sibling = McpFixture::start().await;
        let mut fixture = Fixture::with_session_count(None, 0).await;
        configure(
            &fixture,
            &[("recovering", &recovering), ("sibling", &sibling)],
        );
        let session = create(&fixture).await;
        fixture.sessions.push(session.clone());
        fixture.manager.load(&session.id, None).await.unwrap();
        let runtime = fixture.manager.configuration_runtime(&session.id).unwrap();
        assert!(
            runtime
                .runtime_resources()
                .capability()
                .tool_registry()
                .definitions()
                .iter()
                .all(|tool| tool.name != "recovered_echo")
        );
        recovering.control.fail.store(false, Ordering::SeqCst);
        let completed = Arc::new(Gate::default());
        let release = completed.arm_scoped();
        runtime.install_runtime_refresh_completed_gate(completed.clone());
        let refreshing = runtime.clone();
        let refresh = tokio::spawn(async move { refreshing.refresh_runtime_catalogs().await });
        super::super::gate_entered(&completed).await;
        // The completed recovery owns a valid R. The unrelated authored
        // candidate still sees its original unavailable R authority.
        recovering.control.fail.store(true, Ordering::SeqCst);
        *sibling.control.schema_kind.lock().unwrap() = "integer";
        write(
            &fixture,
            0,
            ConfigMutation::Todo {
                authored: Some(crate::extensions::TodoExtensionDocument { enabled: true }),
            },
        )
        .await;
        let candidate = settled(&fixture, 0).await.candidate.unwrap();
        runtime.wait_for_configuration_admissions().await;
        fixture
            .manager
            .adopt_configuration(&session.id, &candidate.identity, candidate.expected_binding)
            .unwrap();
        let before = *runtime.runtime_catalog_publications().borrow();
        let mut publications = runtime.runtime_catalog_publications();
        drop(release);
        refresh.await.unwrap().unwrap();
        wait_runtime_catalog(&runtime, &mut publications, before).await;
        let definitions = runtime
            .runtime_resources()
            .capability()
            .tool_registry()
            .definitions();
        assert!(definitions.iter().any(|tool| tool.name == "recovered_echo"));
        assert!(definitions.iter().any(|tool| tool.name == "todo"));
        assert!(definitions.iter().any(|tool| tool.name == TOOL));
        assert_eq!(recovering.control.lists.load(Ordering::SeqCst), 3);
        assert_eq!(sibling.control.lists.load(Ordering::SeqCst), 3);
        assert!(fixture.provider.request_bodies().is_empty());
        runtime
            .submit_inbound(input("request-B: rebased complete source inventory"))
            .unwrap();
        fixture.gates[1].wait_entered().await;
        let requests = fixture.provider.request_bodies();
        assert_eq!(requests.len(), 1);
        let request: serde_json::Value = serde_json::from_str(&requests[0]).unwrap();
        assert_eq!(tool_schema(&requests[0]).as_deref(), Some("integer"));
        let tools = request["tools"].as_array().unwrap();
        for name in ["recovered_echo", "todo", TOOL] {
            assert!(tools.iter().any(|tool| tool["function"]["name"] == name));
        }
        let mut settlement = SettlementReceiptWait::capture(&runtime);
        fixture.gates[1].release();
        settlement.wait().await;
        assert_eq!(recovering.control.lists.load(Ordering::SeqCst), 3);
        assert_eq!(sibling.control.lists.load(Ordering::SeqCst), 3);
        fixture.close().await;
        recovering.close().await;
        sibling.close().await;
    })
    .await;
}
