//! Root composition over the real MCP stdio fixture: tool materialization,
//! scripted model turns, and capability availability (mcp-fixture builds).

use std::sync::Arc;

use super::{LocalConversationCore, LocalRuntimeDependencies};
use crate::events::types::{AttemptOutcome, RuntimeEvent};
use crate::launch_fixture::LaunchFixture;
use crate::message::content::TextBlock;
use crate::message::types::{AssistantContentBlock, MessageBlock, UserContentBlock};
use crate::model::event::ModelEvent;
use crate::model::finish::ModelFinishReason;
use crate::model::types::{ModelProtocol, ModelRequest};
use crate::runtime::identity::{McpServerId, ToolCallId, ToolId};
use crate::scripted_suites::support::fake::{FakeModel, FakeStep, fake_model};
use crate::scripted_suites::support::model::{
    FixtureModel, ScriptedAdapterFactory, fixture_registry,
};
use crate::tools::mcp::fixture::{
    ECHO_CALL_COUNT_FILE_ENV, FIXTURE_MODE_ENV, FixtureServer, TOOL_PREFIX_ENV, fixture_spawn_args,
    serve_if_fixture_mode,
};
use crate::tools::types::{ToolCall, ToolCallStart, ToolExecutionStatus};

const SERVER_NAME: &str = "fixture";
const TOOL_NAME: &str = "fixture_echo";
const TEST_AGENT: &str = "explore";

struct ComposedFixture {
    _root: tempfile::TempDir,
    runtime: crate::local_runtime::HeadlessConversationRuntime,
    model: Arc<FakeModel>,
    echo_call_count_file: std::path::PathBuf,
    tool_id: ToolId,
}

fn paths(root: &std::path::Path, workspace: std::path::PathBuf) -> LaunchFixture {
    LaunchFixture {
        config: root.join("rustx.toml"),
        startup_session: super::StartupSession::Empty,
        workspace,
        runtime_root: root.join("runtime"),
    }
}

fn mcp_call_script(tool_id: &ToolId, arguments: serde_json::Value) -> Vec<FakeStep> {
    let call_id = ToolCallId::new("mcp-call-163");
    vec![
        FakeStep::Emit(ModelEvent::Started),
        FakeStep::Emit(ModelEvent::ToolCallStarted {
            block_index: crate::message::types::ContentBlockIndex::new(0),
            call: ToolCallStart {
                id: call_id.clone(),
                tool_id: tool_id.clone(),
                name: TOOL_NAME.to_owned(),
            },
        }),
        FakeStep::Emit(ModelEvent::ToolCallCompleted {
            block_index: crate::message::types::ContentBlockIndex::new(0),
            call: ToolCall {
                id: call_id,
                tool_id: tool_id.clone(),
                name: TOOL_NAME.to_owned(),
                arguments,
            },
        }),
        FakeStep::Emit(ModelEvent::Completed {
            finish_reason: ModelFinishReason::ToolCalls,
            usage: None,
        }),
    ]
}

fn final_answer_script() -> Vec<FakeStep> {
    vec![
        FakeStep::Emit(ModelEvent::Started),
        FakeStep::Emit(ModelEvent::TextDelta {
            block_index: crate::message::types::ContentBlockIndex::new(0),
            text: "done".to_owned(),
        }),
        FakeStep::Emit(ModelEvent::Completed {
            finish_reason: ModelFinishReason::Stop,
            usage: None,
        }),
    ]
}

#[allow(clippy::too_many_lines)] // Composes the real test runtime and its resource fixtures.
async fn compose_fixture(test_name: &str, arguments: serde_json::Value) -> ComposedFixture {
    compose_fixture_with_scripts(test_name, arguments, None, None).await
}

#[allow(clippy::too_many_lines)] // Composes the real test runtime and its resource fixtures.
async fn compose_fixture_with_scripts(
    test_name: &str,
    arguments: serde_json::Value,
    scripts: Option<Vec<Vec<FakeStep>>>,
    mcp_endpoint: Option<&str>,
) -> ComposedFixture {
    let root = tempfile::tempdir().expect("fixture root");
    let workspace = root.path().join("workspace");
    std::fs::create_dir_all(workspace.join(".agents/agents/explore")).expect("workspace");
    std::fs::write(
        workspace.join(".agents/agents/explore.toml"),
        "Read-only fixture subagent instructions.\n",
    )
    .expect("subagent instructions");

    let server_id = McpServerId::new(SERVER_NAME);
    let tool_id = ToolId::new(crate::tools::mcp::mcp_tool_id(&server_id, TOOL_NAME));
    let model = fake_model(
        scripts
            .unwrap_or_else(|| vec![mcp_call_script(&tool_id, arguments), final_answer_script()]),
    );
    let adapter: Arc<dyn crate::model::adapter::ModelAdapter> = model.clone();
    let fixture_model =
        FixtureModel::text("scripted/scripted", ModelProtocol::OpenAiChatCompletions);
    let factory = ScriptedAdapterFactory::new(adapter);
    let registry = fixture_registry(std::slice::from_ref(&fixture_model), &factory);
    let catalog_document = r#"
[providers.scripted]
base_url = "https://scripted.fixture.invalid/v1"
api_key = "test-only-secret"
[models."scripted/scripted"]
provider = "scripted"
id = "scripted"
protocol = "openai_chat_completions"
context_window = 1000000
max_output_tokens = 4096
capabilities = { input_modalities = ["text"], output_modalities = ["text"], tool_calls = true, reasoning = false }
compat = { chat_reasoning_replay = "omit" }
"#;

    let echo_call_count_file = root.path().join("echo-call-count");
    let executable = std::env::current_exe().expect("test executable");
    let mut config_document = serde_json::json!({"schema_version": 10, "agent_id": "agent-parent", "context": {"reserve_tokens": 0, "keep_recent_tokens": 0}, "mcp_servers": {
            SERVER_NAME: {
                "command": executable,
                "args": fixture_spawn_args(test_name),
                "env": {
                    FIXTURE_MODE_ENV: "1",
                    TOOL_PREFIX_ENV: "fixture_",
                    ECHO_CALL_COUNT_FILE_ENV: echo_call_count_file,
                },
            },
        }, "subagents": {"max_concurrent": 4, "roles": {
                TEST_AGENT: {
                    "description": "Read the workspace",

                    "tools": {"builtin": ["read"]},
                },
            }}, "agent": {"model": {"model": "scripted/scripted"}, "tools": {"builtin": ["read"], "sources": {SERVER_NAME: [TOOL_NAME]}}, "agents": [TEST_AGENT]}});
    if let Some(endpoint) = mcp_endpoint {
        config_document["mcp_servers"][SERVER_NAME] = serde_json::json!({"url": endpoint});
    }
    crate::launch_fixture::write_roles(&workspace, &mut config_document["subagents"]);
    let mcp = config_document
        .as_object_mut()
        .unwrap()
        .remove("mcp_servers")
        .unwrap();
    std::fs::write(
        workspace.join(".agents/mcp.toml"),
        toml::to_string_pretty(&serde_json::json!({"mcp_servers": mcp})).unwrap(),
    )
    .unwrap();
    crate::launch_fixture::write_document(
        &root.path().join("rustx.toml"),
        &format!(
            "{}\n{catalog_document}",
            toml::to_string_pretty(&config_document).unwrap()
        ),
    );
    let launch = paths(root.path(), workspace).resolve();
    let runtime_config = launch.config.as_ref().clone();
    let controller = Arc::new(
        crate::runtime::local_storage::ProductController::acquire(&launch.runtime_root).unwrap(),
    );
    let catalog = crate::local_runtime::session::SessionCatalog::create(
        &launch.runtime_root,
        &crate::local_runtime::session::SessionPersistentState::from_input(&launch.input),
    )
    .unwrap();
    let session_id = catalog.persisted_session_ids().remove(0);
    let session = catalog.snapshot(&session_id).unwrap();
    let output_root = catalog
        .database_path(&session.id, &session.active_conversation_id)
        .parent()
        .unwrap()
        .to_path_buf();
    let runtime = LocalConversationCore::compose_from_config(
        &launch,
        &LocalRuntimeDependencies::default(),
        registry,
        runtime_config.clone(),
        super::super::session::SessionPersistentState::from_input(&launch.input),
        session.active_conversation_id,
        output_root,
        controller,
    )
    .await
    .expect("real production composition");

    ComposedFixture {
        _root: root,
        runtime: runtime.into_headless(),
        model,
        echo_call_count_file,
        tool_id,
    }
}

async fn settle(fixture: &ComposedFixture) {
    let settled = fixture.runtime.runtime().settlement_signal().notified();
    fixture
        .runtime
        .runtime()
        .submit_inbound(vec![UserContentBlock::Text(TextBlock {
            text: "use the fixture tool".to_owned(),
        })])
        .expect("inbound accepted");
    tokio::time::timeout(std::time::Duration::from_secs(10), settled)
        .await
        .expect("runtime settlement liveness");
}

fn request_tool_names(request: &ModelRequest) -> Vec<String> {
    request.tools.iter().map(|tool| tool.name.clone()).collect()
}

fn canonical_tool_call(ledger: &[MessageBlock]) -> ToolCall {
    ledger
        .iter()
        .find_map(|message| match message {
            MessageBlock::Assistant(assistant) => assistant.content.iter().find_map(|block| {
                if let AssistantContentBlock::ToolCall(call) = block {
                    Some(call.clone())
                } else {
                    None
                }
            }),
            _ => None,
        })
        .expect("canonical assistant tool call")
}

fn journal(fixture: &ComposedFixture) -> Vec<RuntimeEvent> {
    fixture
        .runtime
        .tool_runtime()
        .durable_store()
        .read_events(None, 256)
        .expect("event journal")
        .events
        .into_iter()
        .map(|event| event.event)
        .collect()
}

/// The real local composition publishes a non-empty named-subagent
/// intrinsic and an MCP fixture tool into one frozen request surface.
/// The parent then calls only MCP, commits its result, and continues; the
/// mere presence of `subagent` never enters child execution.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn named_subagent_and_mcp_share_one_frozen_parent_generation() {
    if serve_if_fixture_mode(FixtureServer::from_env()).await {
        return;
    }
    let test_name = "local_runtime::composition::tests::named_subagent_and_mcp_share_one_frozen_parent_generation";
    let fixture = compose_fixture(test_name, serde_json::json!({})).await;
    settle(&fixture).await;

    let requests = fixture.model.requests();
    assert_eq!(
        requests.len(),
        2,
        "MCP result continues into one next model turn"
    );
    let request_surface = fixture.runtime.runtime().runtime_resources();
    let frozen_tools = request_surface
        .capability()
        .tool_registry()
        .model_definitions();
    assert_eq!(requests[0].tools, frozen_tools);
    assert_eq!(requests[1].tools, requests[0].tools);
    let names = request_tool_names(&requests[0]);
    assert!(names.iter().any(|name| name == "subagent"), "{names:?}");
    assert!(names.iter().any(|name| name == TOOL_NAME), "{names:?}");

    let expected_call = ToolCall {
        id: ToolCallId::new("mcp-call-163"),
        tool_id: fixture.tool_id.clone(),
        name: TOOL_NAME.to_owned(),
        arguments: serde_json::json!({}),
    };
    let ledger = fixture
        .runtime
        .runtime()
        .historical_canonical_history()
        .expect("canonical history");
    assert_eq!(canonical_tool_call(&ledger), expected_call);
    let tool_result = ledger
        .iter()
        .find_map(|message| match message {
            MessageBlock::Tool(tool) => Some(tool),
            _ => None,
        })
        .expect("committed MCP ToolResult");
    assert_eq!(tool_result.tool_call_id, expected_call.id);
    assert_eq!(tool_result.tool_id, expected_call.tool_id);
    assert!(matches!(
        tool_result.result.status,
        ToolExecutionStatus::Success
    ));
    assert_eq!(
        std::fs::read_to_string(&fixture.echo_call_count_file)
            .expect("MCP echo call count")
            .lines()
            .count(),
        1,
        "the MCP fixture executes exactly once"
    );
    assert!(requests[1].messages.iter().any(|message| matches!(
        message,
        crate::model::input::ModelInputMessage::Canonical(MessageBlock::Tool(tool))
            if tool.tool_call_id == expected_call.id
    )));

    let journal = journal(&fixture);
    assert!(matches!(
        journal.last(),
        Some(RuntimeEvent::AttemptCompleted {
            finish_reason: ModelFinishReason::Stop,
            ..
        })
    ));
    assert_eq!(
        journal
            .iter()
            .filter(|event| AttemptOutcome::from_terminal_event(event).is_some())
            .count(),
        1
    );
    assert!(
        fixture
            .runtime
            .runtime()
            .subagent_registry()
            .expect("parent subagent registry")
            .all_snapshots()
            .is_empty()
    );
    assert!(!journal.iter().any(|event| matches!(
        event,
        RuntimeEvent::SubagentOwnershipCommitted { .. }
            | RuntimeEvent::SubagentTerminalPublished { .. }
    )));

    fixture.runtime.shutdown().await.expect("runtime shutdown");
}

/// A structurally complete MCP call with invalid business arguments still
/// crosses canonical assembly and reaches the ordinary strict preflight
/// rejection; the fixture executor is never entered.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn malformed_mcp_arguments_remain_a_normal_preflight_rejection() {
    if serve_if_fixture_mode(FixtureServer::from_env()).await {
        return;
    }
    let test_name = "local_runtime::composition::tests::malformed_mcp_arguments_remain_a_normal_preflight_rejection";
    let fixture = compose_fixture(test_name, serde_json::json!({"unexpected": true})).await;
    settle(&fixture).await;

    let ledger = fixture
        .runtime
        .runtime()
        .historical_canonical_history()
        .expect("canonical history");
    let expected_call_id = ToolCallId::new("mcp-call-163");
    assert_eq!(
        canonical_tool_call(&ledger),
        ToolCall {
            id: expected_call_id.clone(),
            tool_id: fixture.tool_id.clone(),
            name: TOOL_NAME.to_owned(),
            arguments: serde_json::json!({"unexpected": true}),
        }
    );
    let tool_result = ledger
        .iter()
        .find_map(|message| match message {
            MessageBlock::Tool(tool) if tool.tool_call_id == expected_call_id => Some(tool),
            _ => None,
        })
        .expect("preflight ToolResult");
    let ToolExecutionStatus::Failed { error } = &tool_result.result.status else {
        panic!(
            "strict preflight must produce a failed result slot: {:?}",
            tool_result.result.status
        );
    };
    assert!(error.contains("canonical schema"));
    assert!(error.contains("unexpected"));
    assert_eq!(
        std::fs::read_to_string(&fixture.echo_call_count_file)
            .unwrap_or_default()
            .lines()
            .count(),
        0,
        "strict preflight rejects before MCP dispatch"
    );
    let requests = fixture.model.requests();
    assert_eq!(requests.len(), 2, "the normal failed result continues");
    assert!(requests[1].messages.iter().any(|message| matches!(
        message,
        crate::model::input::ModelInputMessage::Canonical(MessageBlock::Tool(tool))
            if tool.tool_call_id == expected_call_id
    )));
    let events = journal(&fixture);
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, RuntimeEvent::ToolExecutionStarted { .. }))
    );
    assert!(events.iter().any(|event| matches!(
        event,
        RuntimeEvent::ToolMessageCommitted { tool_call_id, .. }
            if *tool_call_id == expected_call_id
    )));
    assert!(matches!(
        events.last(),
        Some(RuntimeEvent::AttemptCompleted {
            finish_reason: ModelFinishReason::Stop,
            ..
        })
    ));

    fixture.runtime.shutdown().await.expect("runtime shutdown");
}

/// One real startup catalog whose completion is controlled independently of
/// the model gate. The protocol and composition still use their production path.
#[derive(Clone)]
struct HeadlessStartupServer {
    pause: Arc<crate::tools::mcp::test_sync::ConnectOwnershipPause>,
    lists: Arc<std::sync::atomic::AtomicUsize>,
}

impl rmcp::ServerHandler for HeadlessStartupServer {
    fn get_info(&self) -> rmcp::model::ServerInfo {
        rmcp::model::ServerInfo::new(
            rmcp::model::ServerCapabilities::builder()
                .enable_tools()
                .build(),
        )
    }

    fn discover(
        &self,
        _context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> impl Future<Output = Result<rmcp::model::DiscoverResult, rmcp::ErrorData>> {
        std::future::ready(Ok(rmcp::model::DiscoverResult::from_server_info(
            crate::tools::mcp::supported_protocol_versions(),
            self.get_info(),
        )))
    }

    async fn list_tools(
        &self,
        _request: Option<rmcp::model::PaginatedRequestParams>,
        _context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<rmcp::model::ListToolsResult, rmcp::ErrorData> {
        self.lists.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.pause.park().await;
        Ok(rmcp::model::ListToolsResult {
            tools: vec![crate::tools::mcp::fixture::fixture_tool_named(TOOL_NAME)],
            ..Default::default()
        })
    }
}

async fn headless_startup_server(
    pause: Arc<crate::tools::mcp::test_sync::ConnectOwnershipPause>,
) -> (
    String,
    tokio::task::JoinHandle<()>,
    Arc<std::sync::atomic::AtomicUsize>,
) {
    use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
    use rmcp::transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService,
    };

    let lists = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let observed = lists.clone();
    let service = StreamableHttpService::<HeadlessStartupServer, LocalSessionManager>::new(
        move || {
            Ok(HeadlessStartupServer {
                pause: pause.clone(),
                lists: observed.clone(),
            })
        },
        Arc::default(),
        StreamableHttpServerConfig::default(),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/mcp", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(listener, axum::Router::new().nest_service("/mcp", service))
            .await
            .unwrap();
    });
    (endpoint, task, lists)
}

/// Headless callers explicitly select the ordinary idle adoption boundary;
/// they never acquire a separate mutable/pending tool registry.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(
    clippy::too_many_lines,
    reason = "one gated pending, busy, ready and consumed adoption sequence"
)]
async fn headless_capability_adoption_refuses_busy_and_preserves_admitted_attempt() {
    use crate::runtime::adoption::AdoptionError;

    let startup = Arc::new(crate::tools::mcp::test_sync::ConnectOwnershipPause::default());
    let (endpoint, server, lists) = headless_startup_server(startup.clone()).await;
    let (release, released) = crate::scripted_suites::support::fake::model_release();
    let mut script = vec![FakeStep::ParkUntilReleased(released)];
    script.extend(final_answer_script());
    let fixture = compose_fixture_with_scripts(
        "local_runtime::composition::tests::headless_capability_adoption_refuses_busy_and_preserves_admitted_attempt",
        serde_json::json!({}), Some(vec![script]), Some(&endpoint),
    ).await;
    startup.wait_entered().await;
    let runtime = fixture.runtime.runtime();
    let initial_revision = runtime.runtime_resources().revision();
    assert_eq!(
        fixture.runtime.adopt_ready_capabilities().await,
        Err(AdoptionError::NotReady)
    );
    assert_eq!(runtime.runtime_resources().revision(), initial_revision);
    let mut parked = fixture.model.parked();
    let after = runtime.settlement_ordinal_for_test();
    runtime
        .submit_inbound(vec![UserContentBlock::Text(TextBlock {
            text: "request-A".into(),
        })])
        .unwrap();
    tokio::time::timeout(
        std::time::Duration::from_secs(15),
        parked.wait_for(|parked| *parked),
    )
    .await
    .expect("headless model parks")
    .unwrap();
    let admitted = runtime.configuration_cut().admitted_attempt.unwrap();
    assert_eq!(
        fixture.runtime.adopt_ready_capabilities().await,
        Err(AdoptionError::Busy),
        "busy takes precedence before readiness"
    );
    startup.release();
    let mut readiness = fixture.runtime.capability().startup_readiness().unwrap();
    tokio::time::timeout(
        std::time::Duration::from_secs(15),
        readiness.wait_for(|progress| progress.done),
    )
    .await
    .expect("startup discovery completes")
    .unwrap();
    assert_eq!(
        fixture.runtime.adopt_ready_capabilities().await,
        Err(AdoptionError::Busy)
    );
    assert_eq!(
        runtime.configuration_cut().admitted_attempt.unwrap(),
        admitted
    );
    assert_eq!(fixture.model.requests().len(), 1);
    assert!(!request_tool_names(&fixture.model.requests()[0]).contains(&TOOL_NAME.to_owned()));
    assert_eq!(runtime.runtime_resources().revision(), initial_revision);
    release.send_replace(true);
    tokio::time::timeout(
        std::time::Duration::from_secs(15),
        runtime.child_attempt_settlement(after),
    )
    .await
    .expect("headless attempt settles");
    tokio::time::timeout(
        std::time::Duration::from_secs(15),
        runtime.wait_for_configuration_admissions(),
    )
    .await
    .expect("headless preparation admissions settle");
    tokio::time::timeout(
        std::time::Duration::from_secs(15),
        fixture.runtime.adopt_ready_capabilities(),
    )
    .await
    .expect("headless explicit adoption settles")
    .unwrap();
    let adopted = runtime.runtime_resources();
    assert_ne!(adopted.revision(), initial_revision);
    assert!(
        adopted
            .capability()
            .tool_registry()
            .model_definitions()
            .iter()
            .any(|tool| tool.name == TOOL_NAME)
    );
    assert_eq!(
        fixture.runtime.adopt_ready_capabilities().await,
        Err(AdoptionError::NotReady),
        "a consumed readiness result cannot publish another revision"
    );
    assert_eq!(runtime.runtime_resources().revision(), adopted.revision());
    assert_eq!(
        fixture.model.requests().len(),
        1,
        "adoption never submits a model request"
    );
    assert_eq!(lists.load(std::sync::atomic::Ordering::SeqCst), 1);
    runtime
        .capability()
        .mcp_invalidation()
        .lock()
        .advance(&McpServerId::new(SERVER_NAME));
    assert!(runtime.capability().needs_mcp_refresh());
    fixture.runtime.adopt_ready_capabilities().await.unwrap();
    assert_eq!(lists.load(std::sync::atomic::Ordering::SeqCst), 2);
    assert!(!runtime.capability().needs_mcp_refresh());
    assert_eq!(
        fixture.runtime.adopt_ready_capabilities().await,
        Err(AdoptionError::NotReady)
    );
    assert_eq!(lists.load(std::sync::atomic::Ordering::SeqCst), 2);
    fixture.runtime.shutdown().await.unwrap();
    server.abort();
    let _ = server.await;
}
