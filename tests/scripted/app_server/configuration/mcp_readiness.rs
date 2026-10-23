//! Late live metadata uses the same Session application and adoption gate as
//! authored configuration. The MCP and provider gates determine every ordering.
use super::*;
use crate::capabilities::{CapabilitySourceState, ToolSourceId};
use crate::runtime::identity::McpServerId;
use rmcp::RoleServer;
use rmcp::ServerHandler;
use rmcp::model::{
    DiscoverResult, ListToolsResult, PaginatedRequestParams, ProtocolVersion, ServerCapabilities,
    ServerInfo,
};
use rmcp::service::RequestContext;
use std::sync::atomic::AtomicBool;

const TOOL: &str = "startup_echo";

#[derive(Clone)]
struct StartupServer {
    gate: Arc<super::super::AsyncGate>,
    schema_changed: Arc<AtomicBool>,
    lists: Arc<AtomicUsize>,
    fail: Arc<AtomicBool>,
    name: &'static str,
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
        let mut tool = crate::tools::mcp::fixture::fixture_tool_named(self.name);
        let kind = if self.schema_changed.load(Ordering::SeqCst) {
            "integer"
        } else {
            "string"
        };
        Arc::make_mut(&mut tool.input_schema)
            .insert("properties".into(), serde_json::json!({"id":{"type":kind}}));
        Ok(ListToolsResult {
            tools: vec![tool],
            ..Default::default()
        })
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
            schema_changed: Arc::new(AtomicBool::new(false)),
            lists: Arc::new(AtomicUsize::new(0)),
            fail: Arc::new(AtomicBool::new(false)),
            name,
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cold_readiness_and_warm_schema_change_use_native_adoption_without_mutating_attempt() {
    bounded(async {
        for warm in [false, true] {
            let mut phase = FixturePhase::new(if warm {
                "warm MCP readiness"
            } else {
                "cold MCP readiness"
            });
            let mcp = McpFixture::start().await;
            let mut fixture = Fixture::with_session_count(None, 0).await;
            let workspace = &fixture.workspaces[0];
            std::fs::create_dir_all(workspace.join(".agents")).unwrap();
            std::fs::write(
                workspace.join(".agents/mcp.toml"),
                format!("[mcp_servers.late]\nurl = {:?}\n", mcp.endpoint),
            )
            .unwrap();
            let config = workspace.join("rustx.toml");
            let mut document = std::fs::read_to_string(&config).unwrap();
            document.push_str("\n[agent.tools.sources]\nlate = \"all\"\n");
            std::fs::write(config, document).unwrap();
            if warm {
                let initial = create(&fixture).await;
                fixture.manager.load(&initial.id, None).await.unwrap();
                assert_eq!(mcp.control.lists.load(Ordering::SeqCst), 1);
                fixture
                    .manager
                    .unload(&initial.active_conversation_id)
                    .await
                    .unwrap();
                mcp.control.schema_changed.store(true, Ordering::SeqCst);
            }
            mcp.control.gate.arm();
            let session = create(&fixture).await;
            fixture.sessions.push(session.clone());
            let loading = fixture.load(0);
            phase.enter("MCP list entered");
            mcp.control.gate.entered().await;
            phase.enter("optional load grace");
            loading.await.unwrap().unwrap();
            let runtime = fixture.manager.configuration_runtime(&session.id).unwrap();
            let frozen = runtime.runtime_resources();
            assert_eq!(
                frozen
                    .capability_availability()
                    .get(&ToolSourceId::Mcp(McpServerId::new("late"))),
                Some(&CapabilitySourceState::Connecting { cached: warm })
            );
            runtime.submit_inbound(input("request-A")).unwrap();
            phase.enter("first model request");
            fixture.gates[0].wait_entered().await;
            let admitted = runtime.configuration_cut().admitted_attempt.unwrap();
            assert_eq!(fixture.provider.request_bodies().len(), 1);
            assert_eq!(
                tool_schema(&fixture.provider.request_bodies()[0]),
                warm.then(|| "string".to_owned())
            );

            // A source edit already has its own pending application. Late
            // directory readiness must retain that newer capture rather than
            // reconstructing the older adopted instructions.
            phase.enter("newer instructions application");
            write(
                &fixture,
                0,
                ConfigMutation::Instructions {
                    authored: Some("newer instructions awaiting adoption".into()),
                },
            )
            .await;
            let newer = settled(&fixture, 0).await;
            assert!(newer.candidate.is_some());
            let mut changed = fixture.manager.configuration_changes();
            phase.enter("readiness recaptures newer application");
            mcp.control.gate.release();
            let application = loop {
                if let Some(application) = fixture.manager.configuration_application(&session.id)
                    && application.desired != newer.desired
                    && application
                        .units
                        .values()
                        .all(|unit| !matches!(unit, UnitApplication::Preparing))
                {
                    break application;
                }
                changed.changed().await.unwrap();
            };
            assert_eq!(
                application.desired.input_revision,
                newer.desired.input_revision
            );
            assert!(
                matches!(
                    application.units[&ApplyUnit::Capabilities],
                    UnitApplication::Ready { .. }
                ),
                "{application:?}"
            );
            let candidate = application.candidate.unwrap();
            assert_eq!(
                runtime.configuration_cut().admitted_attempt.unwrap(),
                admitted
            );
            assert_eq!(runtime.runtime_resources().revision(), frozen.revision());
            assert_eq!(
                fixture.manager.adopt_configuration(
                    &session.id,
                    &candidate.identity,
                    candidate.expected_binding
                ),
                Err(AdoptionError::Busy)
            );
            assert_eq!(
                fixture.provider.request_bodies().len(),
                1,
                "readiness never invokes a model"
            );
            assert_eq!(
                mcp.control.lists.load(Ordering::SeqCst),
                if warm { 2 } else { 1 },
                "native readiness reuses completed startup discovery"
            );
            let mut settlement = SettlementReceiptWait::capture(&runtime);
            phase.enter("first attempt settlement");
            fixture.gates[0].release();
            settlement.wait().await;
            phase.enter("configuration preparation settlement");
            runtime.wait_for_configuration_admissions().await;
            phase.enter("explicit adoption");
            fixture
                .manager
                .adopt_configuration(&session.id, &candidate.identity, candidate.expected_binding)
                .unwrap();
            assert_eq!(
                runtime
                    .runtime_resources()
                    .configuration()
                    .unwrap()
                    .config
                    .agent
                    .instructions,
                "newer instructions awaiting adoption"
            );
            runtime.submit_inbound(input("request-B")).unwrap();
            phase.enter("second model request");
            fixture.gates[1].wait_entered().await;
            let requests = fixture.provider.request_bodies();
            assert_eq!(requests.len(), 2);
            assert_eq!(tool_schema(&requests[0]), warm.then(|| "string".to_owned()));
            assert_eq!(
                tool_schema(&requests[1]).as_deref(),
                Some(if warm { "integer" } else { "string" })
            );
            let mut settlement = SettlementReceiptWait::capture(&runtime);
            fixture.gates[1].release();
            phase.enter("second attempt settlement");
            settlement.wait().await;
            phase.enter("runtime shutdown");
            fixture.close().await;
            mcp.close().await;
            phase.finish();
        }
    })
    .await;
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

async fn ready_application(
    fixture: &Fixture,
    session: &SessionSnapshot,
) -> ConfigurationApplication {
    let mut changes = fixture.manager.configuration_changes();
    loop {
        if let Some(application) = fixture.manager.configuration_application(&session.id)
            && application.candidate.is_some()
            && application
                .units
                .values()
                .all(|unit| !matches!(unit, UnitApplication::Preparing))
        {
            assert!(
                matches!(
                    application.units[&ApplyUnit::Capabilities],
                    UnitApplication::Ready { .. }
                ),
                "{application:?}"
            );
            return application;
        }
        changes.changed().await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn each_late_source_can_be_adopted_while_a_sibling_remains_held() {
    bounded(async {
        let mut phase = FixturePhase::new("independent late MCP readiness");
        let first = McpFixture::named("first_echo").await;
        let sibling = McpFixture::named("sibling_echo").await;
        let mut fixture = Fixture::with_session_count(None, 0).await;
        configure(&fixture, &[("first", &first), ("sibling", &sibling)]);
        first.control.gate.arm();
        sibling.control.gate.arm();
        let session = create(&fixture).await;
        fixture.sessions.push(session.clone());
        let loading = fixture.load(0);
        phase.enter("both startup listings entered");
        first.control.gate.entered().await;
        sibling.control.gate.entered().await;
        loading.await.unwrap().unwrap();
        let runtime = fixture.manager.configuration_runtime(&session.id).unwrap();
        let before = runtime.runtime_resources();
        first.control.gate.release();
        phase.enter("first source ready with sibling held");
        let ready = ready_application(&fixture, &session)
            .await
            .candidate
            .unwrap();
        runtime.wait_for_configuration_admissions().await;
        assert_eq!(runtime.runtime_resources().revision(), before.revision());
        assert_eq!(sibling.control.gate.parked_now(), 1);
        fixture
            .manager
            .adopt_configuration(&session.id, &ready.identity, ready.expected_binding)
            .unwrap();
        let adopted = runtime.runtime_resources();
        assert_eq!(
            adopted
                .capability_availability()
                .get(&ToolSourceId::Mcp(McpServerId::new("first"))),
            Some(&CapabilitySourceState::Ready)
        );
        assert_eq!(
            adopted
                .capability_availability()
                .get(&ToolSourceId::Mcp(McpServerId::new("sibling"))),
            Some(&CapabilitySourceState::Connecting { cached: false })
        );
        assert_eq!(first.control.lists.load(Ordering::SeqCst), 1);
        assert_eq!(sibling.control.lists.load(Ordering::SeqCst), 1);
        runtime.submit_inbound(input("first source only")).unwrap();
        phase.enter("request uses independently adopted source");
        fixture.gates[0].wait_entered().await;
        let body: serde_json::Value =
            serde_json::from_str(&fixture.provider.request_bodies()[0]).unwrap();
        let tools = body["tools"].as_array().unwrap();
        assert!(
            tools
                .iter()
                .any(|tool| tool["function"]["name"] == "first_echo")
        );
        assert!(
            !tools
                .iter()
                .any(|tool| tool["function"]["name"] == "sibling_echo")
        );
        let mut settlement = SettlementReceiptWait::capture(&runtime);
        fixture.gates[0].release();
        settlement.wait().await;
        phase.enter("drain cancels still pending sibling");
        fixture.close().await;
        sibling.control.gate.release();
        first.close().await;
        sibling.close().await;
        phase.finish();
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn warm_startup_failure_becomes_unavailable_through_native_adoption() {
    bounded(async {
        let mut phase = FixturePhase::new("warm terminal MCP failure");
        let mcp = McpFixture::start().await;
        let mut fixture = Fixture::with_session_count(None, 0).await;
        configure(&fixture, &[("late", &mcp)]);
        let initial = create(&fixture).await;
        fixture.manager.load(&initial.id, None).await.unwrap();
        fixture
            .manager
            .unload(&initial.active_conversation_id)
            .await
            .unwrap();
        mcp.control.fail.store(true, Ordering::SeqCst);
        mcp.control.gate.arm();
        let session = create(&fixture).await;
        fixture.sessions.push(session.clone());
        let loading = fixture.load(0);
        mcp.control.gate.entered().await;
        loading.await.unwrap().unwrap();
        let runtime = fixture.manager.configuration_runtime(&session.id).unwrap();
        runtime
            .submit_inbound(input("request-A: frozen warm request"))
            .unwrap();
        phase.enter("warm request admitted");
        fixture.gates[0].wait_entered().await;
        let frozen = runtime.configuration_cut().admitted_attempt.unwrap();
        assert_eq!(
            tool_schema(&fixture.provider.request_bodies()[0]).as_deref(),
            Some("string")
        );
        mcp.control.gate.release();
        phase.enter("terminal failure prepares native candidate");
        let ready = ready_application(&fixture, &session)
            .await
            .candidate
            .unwrap();
        assert_eq!(
            runtime.configuration_cut().admitted_attempt.unwrap(),
            frozen
        );
        assert_eq!(
            runtime
                .runtime_resources()
                .capability_availability()
                .get(&ToolSourceId::Mcp(McpServerId::new("late"))),
            Some(&CapabilitySourceState::Connecting { cached: true })
        );
        let mut settlement = SettlementReceiptWait::capture(&runtime);
        fixture.gates[0].release();
        settlement.wait().await;
        runtime.wait_for_configuration_admissions().await;
        phase.enter("adopt terminal failure");
        fixture
            .manager
            .adopt_configuration(&session.id, &ready.identity, ready.expected_binding)
            .unwrap();
        assert!(matches!(
            runtime
                .runtime_resources()
                .capability_availability()
                .get(&ToolSourceId::Mcp(McpServerId::new("late"))),
            Some(CapabilitySourceState::Unavailable { .. })
        ));
        phase.enter("submit request after terminal adoption");
        runtime
            .submit_inbound(input("request-B: after failure adoption"))
            .unwrap();
        phase.enter("second model request after terminal adoption");
        fixture.gates[1].wait_entered().await;
        assert!(tool_schema(&fixture.provider.request_bodies()[1]).is_none());
        assert_eq!(
            mcp.control.lists.load(Ordering::SeqCst),
            2,
            "failure is observed without retrying discovery"
        );
        phase.enter("second attempt settlement after terminal adoption");
        let mut settlement = SettlementReceiptWait::capture(&runtime);
        fixture.gates[1].release();
        settlement.wait().await;
        phase.enter("shutdown after warm failure");
        fixture.close().await;
        mcp.close().await;
        phase.finish();
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn invalidated_startup_listing_becomes_unavailable_after_native_adoption() {
    bounded(async {
        let mut phase = FixturePhase::new("invalidated startup listing");
        let mcp = McpFixture::start().await;
        let mut fixture = Fixture::with_session_count(None, 0).await;
        configure(&fixture, &[("late", &mcp)]);
        let initial = create(&fixture).await;
        fixture.manager.load(&initial.id, None).await.unwrap();
        fixture
            .manager
            .unload(&initial.active_conversation_id)
            .await
            .unwrap();
        mcp.control.gate.arm();
        let session = create(&fixture).await;
        fixture.sessions.push(session.clone());
        let loading = fixture.load(0);
        phase.enter("startup list held after capture");
        mcp.control.gate.entered().await;
        loading.await.unwrap().unwrap();
        let runtime = fixture.manager.configuration_runtime(&session.id).unwrap();
        let id = McpServerId::new("late");
        let source = ToolSourceId::Mcp(id.clone());
        let original = runtime.runtime_resources();
        assert_eq!(
            original.capability_availability().get(&source),
            Some(&CapabilitySourceState::Connecting { cached: true })
        );
        // Exactly the boundary used by a tools/list_changed notification:
        // discovery owns the old epoch while the server still holds its reply.
        let invalidation = runtime.capability().mcp_invalidation();
        let before = invalidation.epoch(&id);
        invalidation.lock().advance(&id);
        assert_eq!(invalidation.epoch(&id), before + 1);
        mcp.control.gate.release();
        phase.enter("invalidated discovery prepares terminal state");
        let ready = ready_application(&fixture, &session)
            .await
            .candidate
            .unwrap();
        assert_eq!(runtime.runtime_resources().revision(), original.revision());
        runtime.wait_for_configuration_admissions().await;
        fixture
            .manager
            .adopt_configuration(&session.id, &ready.identity, ready.expected_binding)
            .unwrap();
        let adopted = runtime.runtime_resources();
        let Some(CapabilitySourceState::Unavailable { reason }) =
            adopted.capability_availability().get(&source)
        else {
            panic!("invalidated completed listing must not remain Connecting");
        };
        assert!(reason.contains("changed during discovery"), "{reason}");
        assert_eq!(
            original.capability_availability().get(&source),
            Some(&CapabilitySourceState::Connecting { cached: true })
        );
        assert_eq!(
            mcp.control.lists.load(Ordering::SeqCst),
            2,
            "terminal discovery is not retried by readiness publication"
        );
        phase.enter("shutdown after terminal adoption");
        fixture.close().await;
        mcp.close().await;
        phase.finish();
    })
    .await;
}
