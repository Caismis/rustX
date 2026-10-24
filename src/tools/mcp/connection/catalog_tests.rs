//! Real HTTP catalog validation with exact list/dispatch barriers and counters.
use super::*;
use crate::runtime::identity::{ConversationId, ToolCallId};
use crate::tools::executor::{ProgressReporter, ToolExecutionContext};
use crate::tools::mcp::{
    CanonicalMcpTool, McpRuntimeGeneration, McpRuntimeRetirementRegistry, McpTransportConfig,
};
use crate::tools::runtime::ConversationToolRuntime;
use crate::tools::types::{ToolInvocation, ToolInvocationId, ToolInvocationMode};
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, DiscoverResult, ListToolsResult,
    PaginatedRequestParams, ServerCapabilities, ServerInfo, Tool,
};
use rmcp::service::RequestContext;
use rmcp::{RoleServer, ServerHandler};
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::watch;

struct Silent;
impl ProgressReporter for Silent {
    fn report(&self, _: crate::tools::types::ToolProgress) {}
}

#[derive(Clone)]
struct Peer {
    tool: Arc<Mutex<Tool>>,
    lists: watch::Sender<usize>,
    release: watch::Sender<bool>,
    calls: Arc<AtomicUsize>,
    handshakes: Arc<AtomicUsize>,
    fail: Arc<std::sync::atomic::AtomicBool>,
    duplicate: Arc<std::sync::atomic::AtomicBool>,
}

impl ServerHandler for Peer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
    }
    fn discover(
        &self,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<DiscoverResult, rmcp::ErrorData>> {
        self.handshakes.fetch_add(1, Ordering::SeqCst);
        std::future::ready(Ok(DiscoverResult::from_server_info(
            crate::tools::mcp::supported_protocol_versions(),
            self.get_info(),
        )))
    }
    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, rmcp::ErrorData> {
        // Capture before publishing the barrier so invalidation races a concrete response.
        let tool = self.tool.lock().unwrap().clone();
        self.lists.send_modify(|count| *count += 1);
        let mut release = self.release.subscribe();
        tokio::select! {
            _ = release.wait_for(|released| *released) => {},
            () = context.ct.cancelled() => return Err(rmcp::ErrorData::internal_error("request cancelled", None)),
        }
        if self.fail.load(Ordering::SeqCst) {
            return Err(rmcp::ErrorData::internal_error(
                "fresh listing failed",
                None,
            ));
        }
        let mut tools = vec![tool];
        if self.duplicate.load(Ordering::SeqCst) {
            let mut duplicate = tools[0].clone();
            duplicate.description = Some("ambiguous duplicate".into());
            tools.push(duplicate);
        }
        Ok(ListToolsResult {
            tools,
            ttl_ms: Some(3_600_000),
            ..Default::default()
        })
    }
    fn call_tool(
        &self,
        _request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<CallToolResponse, rmcp::ErrorData>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        std::future::ready(Ok(CallToolResult::success(vec![]).into()))
    }
}

struct Fixture {
    peer: Peer,
    binding: McpServerBinding,
    server: tokio::task::JoinHandle<()>,
    shutdown: tokio_util::sync::CancellationToken,
    _temp: tempfile::TempDir,
    tools: ConversationToolRuntime,
}

impl Fixture {
    async fn new() -> Self {
        use rmcp::transport::streamable_http_server::{
            StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
        };
        let peer = Peer {
            tool: Arc::new(Mutex::new(tool("string"))),
            lists: watch::channel(0).0,
            release: watch::channel(true).0,
            calls: Arc::default(),
            handshakes: Arc::default(),
            fail: Arc::default(),
            duplicate: Arc::default(),
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
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        std::fs::create_dir(&workspace).unwrap();
        let tools = ConversationToolRuntime::new(
            ConversationId::new("conv_14dafca7-8c38-7c20-8ac1-caa3300d9cab"),
            workspace,
            temp.path().join("artifacts"),
        )
        .unwrap();
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
            _temp: temp,
            tools,
        }
    }
    fn connection(&self) -> Arc<McpConnection> {
        McpConnection::lazy(
            McpServerId::new("catalog-test"),
            self.binding.clone(),
            self.tools.workspace().clone(),
            Arc::default(),
            CancellationSignal::new(),
        )
    }
    async fn wait_lists(&self, count: usize) {
        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            self.peer.lists.subscribe().wait_for(|seen| *seen >= count),
        )
        .await
        .unwrap()
        .unwrap();
    }
    fn context(&self, signal: CancellationSignal) -> ToolExecutionContext<'_> {
        ToolExecutionContext::new(
            self.tools.conversation_id(),
            None,
            crate::runtime::ExecutionCancellation::detached(
                signal,
                crate::runtime::types::CancellationReason::UserRequested,
            ),
            self.tools.workspace(),
            &Silent,
            self.tools.artifacts(),
            self.tools.tool_output(),
            self.tools.environment(),
        )
    }
    async fn execute(
        &self,
        connection: Arc<McpConnection>,
        expected: CanonicalMcpTool,
    ) -> crate::tools::types::ToolExecutionResult {
        self.execute_with_cancellation(connection, expected, CancellationSignal::new())
            .await
    }
    async fn execute_with_cancellation(
        &self,
        connection: Arc<McpConnection>,
        expected: CanonicalMcpTool,
        signal: CancellationSignal,
    ) -> crate::tools::types::ToolExecutionResult {
        let retirement = McpRuntimeRetirementRegistry::new();
        let generation = McpRuntimeGeneration::from_connection(
            McpServerId::new("catalog-test"),
            connection.clone(),
            None,
            tokio::runtime::Handle::current(),
            &retirement,
        );
        let entries = crate::tools::mcp::executor::definitions_owned(
            &McpServerId::new("catalog-test"),
            self.binding.policy,
            &generation.binding(),
            vec![expected],
        );
        let (definition, executor) = &entries[0];
        let context = self.context(signal);
        let result = executor
            .start(
                ToolInvocation {
                    id: ToolInvocationId::Agent {
                        call_id: ToolCallId::new("catalog-call"),
                    },
                    tool_id: definition.id.clone(),
                    tool_name: definition.name.clone(),
                    mode: ToolInvocationMode::Foreground,
                    arguments: serde_json::json!({}),
                },
                context,
            )
            .completion
            .await;
        if let Some(runtime) = connection.published_runtime()
            && let Some(ownership) = &runtime.request_ownership
        {
            assert_eq!(
                ownership.outstanding_requests(),
                0,
                "invocation must release every local HTTP request before connection retirement"
            );
        }
        assert!(generation.retire_and_close().await.is_none());
        result
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.shutdown.cancel();
        self.server.abort();
    }
}
fn tool(kind: &str) -> Tool {
    let mut tool = crate::tools::mcp::fixture::fixture_tool_named("lookup");
    tool.input_schema = Arc::new(
        serde_json::json!({"type":"object","properties":{"id":{"type":kind}}})
            .as_object()
            .unwrap()
            .clone(),
    );
    tool
}

#[tokio::test]
async fn independent_connection_first_use_waits_for_its_own_live_schema() {
    let fixture = Fixture::new().await;
    let first = fixture.connection();
    let expected = first.discover_catalog().await.unwrap().remove(0);
    assert!(first.close().await.is_empty());
    let second = fixture.connection();
    assert_eq!(second.established_count().await, 0);
    fixture.peer.release.send_replace(false);
    let execution = fixture.execute(second, expected);
    tokio::pin!(execution);
    tokio::select! {
        _ = &mut execution => panic!("dispatch must wait for live listing"),
        () = fixture.wait_lists(2) => {},
    }
    assert_eq!(fixture.peer.calls.load(Ordering::SeqCst), 0);
    fixture.peer.release.send_replace(true);
    assert!(matches!(
        execution.await.status,
        crate::tools::types::ToolExecutionStatus::Success
    ));
    assert_eq!(fixture.peer.calls.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.peer.handshakes.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn same_name_description_changed_schema_never_dispatches_frozen_tool() {
    let fixture = Fixture::new().await;
    let first = fixture.connection();
    let expected = first.discover_catalog().await.unwrap().remove(0);
    assert!(first.close().await.is_empty());
    *fixture.peer.tool.lock().unwrap() = tool("integer");
    let result = fixture.execute(fixture.connection(), expected).await;
    assert!(matches!(
        result.status,
        crate::tools::types::ToolExecutionStatus::Failed { .. }
    ));
    assert_eq!(fixture.peer.calls.load(Ordering::SeqCst), 0);
    assert_eq!(*fixture.peer.lists.borrow(), 2);
}

#[tokio::test]
async fn schema_mismatch_preserves_new_live_catalog_for_native_publication() {
    let fixture = Fixture::new().await;
    let connection = fixture.connection();
    let authority = connection.reconnect.as_ref().unwrap();
    let mut changes = authority.invalidation.subscribe_catalog_changes();
    assert_eq!(*changes.borrow_and_update(), 0);
    let frozen = connection.discover_catalog().await.unwrap().remove(0);
    assert_eq!(*changes.borrow_and_update(), 1);
    connection.discover_catalog().await.unwrap();
    assert!(
        !changes.has_changed().unwrap(),
        "coalesced reads are not discoveries"
    );
    *fixture.peer.tool.lock().unwrap() = tool("integer");
    authority.invalidation.lock().advance(&connection.server_id);
    let epoch = connection.catalog_epoch();
    let context = fixture.context(CancellationSignal::new());
    assert!(connection.validate_tool(&frozen, &context).await.is_err());
    assert_eq!(fixture.peer.calls.load(Ordering::SeqCst), 0);
    assert_eq!(*changes.borrow_and_update(), 2);
    let (observed_epoch, catalog) = connection.catalog_snapshot().unwrap();
    assert_eq!(observed_epoch, epoch);
    assert_eq!(
        connection.catalog_epoch(),
        epoch,
        "frozen mismatch does not revoke the live capture"
    );
    let adopted = catalog.unwrap().remove(0);
    assert_eq!(adopted.input_schema["properties"]["id"]["type"], "integer");
    assert!(matches!(
        fixture.execute(connection.clone(), adopted).await.status,
        crate::tools::types::ToolExecutionStatus::Success
    ));
    assert_eq!(fixture.peer.calls.load(Ordering::SeqCst), 1);
    assert_eq!(*fixture.peer.lists.borrow(), 2);
}

#[tokio::test]
async fn invalidation_during_listing_rejects_capture_and_sdk_ttl_never_hides_failure() {
    let fixture = Fixture::new().await;
    let connection = fixture.connection();
    connection.discover_catalog().await.unwrap();
    fixture.peer.release.send_replace(false);
    let authority = connection.reconnect.as_ref().unwrap();
    authority.invalidation.lock().advance(&connection.server_id);
    let discovery = connection.discover_catalog();
    tokio::pin!(discovery);
    tokio::select! {
        _ = &mut discovery => panic!("listing is held"),
        () = fixture.wait_lists(2) => {},
    }
    authority.invalidation.lock().advance(&connection.server_id);
    fixture.peer.release.send_replace(true);
    assert!(discovery.await.is_err());
    fixture.peer.fail.store(true, Ordering::SeqCst);
    assert!(connection.discover_catalog().await.is_err());
    assert_eq!(*fixture.peer.lists.borrow(), 3);
    assert!(connection.close().await.is_empty());
}

#[tokio::test]
async fn replacement_transport_revalidates_schema_and_coalesces_concurrent_fetches() {
    let fixture = Fixture::new().await;
    let connection = fixture.connection();
    let (first, second) =
        tokio::join!(connection.discover_catalog(), connection.discover_catalog());
    assert_eq!(first.as_ref().unwrap(), second.as_ref().unwrap());
    assert_eq!(*fixture.peer.lists.borrow(), 1);
    connection
        .acquire()
        .await
        .unwrap()
        .runtime()
        .close()
        .await
        .unwrap();
    *fixture.peer.tool.lock().unwrap() = tool("integer");
    assert!(matches!(
        fixture
            .execute(connection.clone(), first.unwrap().remove(0))
            .await
            .status,
        crate::tools::types::ToolExecutionStatus::Failed { .. }
    ));
    assert_eq!(*fixture.peer.lists.borrow(), 2);
    assert_eq!(fixture.peer.handshakes.load(Ordering::SeqCst), 2);
    assert!(connection.close().await.is_empty());
}

#[tokio::test]
async fn acquired_catalog_waiters_share_one_complete_live_discovery() {
    let fixture = Fixture::new().await;
    let connection = fixture.connection();
    let transport = connection.acquire().await.unwrap();
    fixture.peer.release.send_replace(false);
    let discovering = async {
        tokio::join!(
            connection.discover_acquired_catalog(transport.clone()),
            connection.discover_acquired_catalog(transport),
        )
    };
    tokio::pin!(discovering);
    tokio::select! {
        _ = &mut discovering => panic!("the complete live listing is held"),
        () = fixture.wait_lists(1) => {},
    }
    assert_eq!(fixture.peer.handshakes.load(Ordering::SeqCst), 1);
    assert_eq!(*fixture.peer.lists.borrow(), 1);
    fixture.peer.release.send_replace(true);
    let (first, second) = discovering.await;
    assert_eq!(first.as_ref().unwrap(), second.as_ref().unwrap());
    assert_eq!(
        first.unwrap(),
        vec![CanonicalMcpTool::try_from(tool("string")).unwrap()]
    );
    assert_eq!(fixture.peer.handshakes.load(Ordering::SeqCst), 1);
    assert_eq!(*fixture.peer.lists.borrow(), 1);
    assert_eq!(fixture.peer.calls.load(Ordering::SeqCst), 0);
    assert!(connection.close().await.is_empty());
}

#[tokio::test]
async fn invalidation_after_validation_before_enqueue_refuses_tools_call() {
    let fixture = Fixture::new().await;
    let connection = fixture.connection();
    let expected = connection.discover_catalog().await.unwrap().remove(0);
    let pause = Arc::new(crate::tools::mcp::test_sync::ConnectOwnershipPause::default());
    *connection.catalog.validated_pause.lock().unwrap() = Some(pause.clone());
    let execution = fixture.execute(connection.clone(), expected);
    tokio::pin!(execution);
    tokio::select! {
        _ = &mut execution => panic!("validation checkpoint is held"),
        () = pause.wait_entered() => {},
    }
    connection
        .reconnect
        .as_ref()
        .unwrap()
        .invalidation
        .lock()
        .advance(&connection.server_id);
    pause.release();
    assert!(matches!(
        execution.await.status,
        crate::tools::types::ToolExecutionStatus::Failed { .. }
    ));
    assert_eq!(fixture.peer.calls.load(Ordering::SeqCst), 0);
    assert_eq!(*fixture.peer.lists.borrow(), 1);
}

#[tokio::test]
async fn newer_catalog_cannot_authorize_an_older_validation_capture() {
    let fixture = Fixture::new().await;
    let connection = fixture.connection();
    let expected = connection.discover_catalog().await.unwrap().remove(0);
    let pause = Arc::new(crate::tools::mcp::test_sync::ConnectOwnershipPause::default());
    *connection.catalog.before_validate_pause.lock().unwrap() = Some(pause.clone());
    let execution = fixture.execute(connection.clone(), expected);
    tokio::pin!(execution);
    tokio::select! {
        _ = &mut execution => panic!("capture checkpoint is held"),
        () = pause.wait_entered() => {},
    }
    *fixture.peer.tool.lock().unwrap() = tool("integer");
    connection
        .reconnect
        .as_ref()
        .unwrap()
        .invalidation
        .lock()
        .advance(&connection.server_id);
    connection.discover_catalog().await.unwrap();
    pause.release();
    assert!(matches!(
        execution.await.status,
        crate::tools::types::ToolExecutionStatus::Failed { .. }
    ));
    assert_eq!(fixture.peer.calls.load(Ordering::SeqCst), 0);
    assert_eq!(*fixture.peer.lists.borrow(), 2);
}

/// One invocation may establish one transport, but cannot replace that same
/// transport again if it dies after listing and before frozen-tool validation.
#[tokio::test]
async fn transport_lost_after_discovery_waits_for_the_next_invocation_to_reconnect() {
    let fixture = Fixture::new().await;
    let connection = fixture.connection();
    let expected = CanonicalMcpTool::try_from(tool("string")).unwrap();
    let retirement = McpRuntimeRetirementRegistry::new();
    let generation = McpRuntimeGeneration::from_connection(
        McpServerId::new("catalog-test"),
        connection.clone(),
        None,
        tokio::runtime::Handle::current(),
        &retirement,
    );
    let entries = crate::tools::mcp::executor::definitions_owned(
        &McpServerId::new("catalog-test"),
        fixture.binding.policy,
        &generation.binding(),
        vec![expected],
    );
    let (definition, executor) = &entries[0];
    let invocation = |call_id: &str| ToolInvocation {
        id: ToolInvocationId::Agent {
            call_id: ToolCallId::new(call_id),
        },
        tool_id: definition.id.clone(),
        tool_name: definition.name.clone(),
        mode: ToolInvocationMode::Foreground,
        arguments: serde_json::json!({}),
    };
    let pause = Arc::new(crate::tools::mcp::test_sync::ConnectOwnershipPause::default());
    *connection.catalog.before_validate_pause.lock().unwrap() = Some(pause.clone());
    let first = executor
        .start(
            invocation("first-connect-owner"),
            fixture.context(CancellationSignal::new()),
        )
        .completion;
    tokio::pin!(first);
    tokio::select! {
        _ = &mut first => panic!("the first transport's catalog capture is held"),
        () = pause.wait_entered() => {},
    }
    assert_eq!(connection.established_count().await, 1);
    assert_eq!(fixture.peer.handshakes.load(Ordering::SeqCst), 1);
    assert_eq!(*fixture.peer.lists.borrow(), 1);
    let lost = connection.published_runtime().unwrap();
    lost.close().await.unwrap();
    pause.release();
    let result = tokio::time::timeout(std::time::Duration::from_secs(10), first)
        .await
        .unwrap();
    assert!(matches!(
        result.status,
        crate::tools::types::ToolExecutionStatus::Failed { .. }
    ));
    assert_eq!(connection.established_count().await, 1);
    assert_eq!(fixture.peer.handshakes.load(Ordering::SeqCst), 1);
    assert_eq!(*fixture.peer.lists.borrow(), 1);
    assert_eq!(fixture.peer.calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        lost.request_ownership
            .as_ref()
            .unwrap()
            .outstanding_requests(),
        0
    );

    let result = executor
        .start(
            invocation("next-connect-owner"),
            fixture.context(CancellationSignal::new()),
        )
        .completion
        .await;
    assert!(matches!(
        result.status,
        crate::tools::types::ToolExecutionStatus::Success
    ));
    assert_eq!(connection.established_count().await, 2);
    assert_eq!(fixture.peer.handshakes.load(Ordering::SeqCst), 2);
    assert_eq!(*fixture.peer.lists.borrow(), 2);
    assert_eq!(fixture.peer.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        connection
            .published_runtime()
            .unwrap()
            .request_ownership
            .as_ref()
            .unwrap()
            .outstanding_requests(),
        0,
    );
    assert!(generation.retire_and_close().await.is_none());
}

#[tokio::test]
async fn duplicate_live_names_never_authorize_a_frozen_declaration() {
    let fixture = Fixture::new().await;
    let connection = fixture.connection();
    let expected = connection.discover_catalog().await.unwrap().remove(0);
    fixture.peer.duplicate.store(true, Ordering::SeqCst);
    connection
        .reconnect
        .as_ref()
        .unwrap()
        .invalidation
        .lock()
        .advance(&connection.server_id);
    let result = fixture.execute(connection, expected).await;
    assert!(matches!(
        result.status,
        crate::tools::types::ToolExecutionStatus::Failed { .. }
    ));
    assert_eq!(fixture.peer.calls.load(Ordering::SeqCst), 0);
    assert_eq!(*fixture.peer.lists.borrow(), 2);
}

#[tokio::test]
async fn cancellation_during_live_listing_settles_without_dispatching_a_tool() {
    let fixture = Fixture::new().await;
    let connection = fixture.connection();
    let expected = connection.discover_catalog().await.unwrap().remove(0);
    connection
        .reconnect
        .as_ref()
        .unwrap()
        .invalidation
        .lock()
        .advance(&connection.server_id);
    fixture.peer.release.send_replace(false);
    let signal = CancellationSignal::new();
    let execution = fixture.execute_with_cancellation(connection, expected, signal.clone());
    tokio::pin!(execution);
    tokio::select! {
        _ = &mut execution => panic!("live listing is held"),
        () = fixture.wait_lists(2) => {},
    }
    signal.cancel();
    let result = tokio::time::timeout(std::time::Duration::from_secs(10), execution)
        .await
        .unwrap();
    assert!(matches!(
        result.status,
        crate::tools::types::ToolExecutionStatus::Cancelled { .. }
    ));
    assert_eq!(fixture.peer.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn cancellation_while_waiting_for_discovery_lock_starts_no_request() {
    let fixture = Fixture::new().await;
    let connection = fixture.connection();
    let expected = connection.discover_catalog().await.unwrap().remove(0);
    let discovery = connection.catalog.discovery.lock().await;
    let signal = CancellationSignal::new();
    let execution = fixture.execute_with_cancellation(connection.clone(), expected, signal.clone());
    tokio::pin!(execution);
    assert!(futures_util::FutureExt::now_or_never(execution.as_mut()).is_none());
    signal.cancel();
    let result = tokio::time::timeout(std::time::Duration::from_secs(10), execution)
        .await
        .unwrap();
    assert!(matches!(
        result.status,
        crate::tools::types::ToolExecutionStatus::Cancelled { .. }
    ));
    assert_eq!(*fixture.peer.lists.borrow(), 1);
    assert_eq!(fixture.peer.calls.load(Ordering::SeqCst), 0);
    drop(discovery);
}

#[tokio::test]
async fn cancellation_while_acquiring_connection_starts_no_request() {
    let fixture = Fixture::new().await;
    let connection = fixture.connection();
    let state = connection.state.lock().await;
    let signal = CancellationSignal::new();
    let cancellation = crate::runtime::ExecutionCancellation::detached(
        signal.clone(),
        crate::runtime::types::CancellationReason::UserRequested,
    );
    let acquiring = connection.acquire_owned(Some(&cancellation));
    tokio::pin!(acquiring);
    assert!(futures_util::FutureExt::now_or_never(acquiring.as_mut()).is_none());
    signal.cancel();
    assert!(acquiring.await.is_err());
    assert_eq!(fixture.peer.handshakes.load(Ordering::SeqCst), 0);
    drop(state);
    assert!(connection.close().await.is_empty());
}

#[tokio::test]
async fn catalog_observation_retains_capture_epoch_and_terminal_invalidation_failure() {
    let fixture = Fixture::new().await;
    let connection = fixture.connection();
    connection.discover_catalog().await.unwrap();
    let (captured, tools) = connection.catalog_snapshot().unwrap();
    let authority = connection.reconnect.as_ref().unwrap();
    authority.invalidation.lock().advance(&connection.server_id);
    assert_ne!(captured, connection.catalog_epoch());
    assert!(tools.is_ok());
    assert!(connection.catalog_snapshot().unwrap().1.is_err());
    fixture.peer.release.send_replace(false);
    let discovery = connection.discover_catalog();
    tokio::pin!(discovery);
    tokio::select! {
        _ = &mut discovery => panic!("listing is held"),
        () = fixture.wait_lists(2) => {},
    }
    authority.invalidation.lock().advance(&connection.server_id);
    fixture.peer.release.send_replace(true);
    assert!(discovery.await.is_err());
    let (failed_epoch, outcome) = connection.catalog_snapshot().unwrap();
    assert_eq!(failed_epoch, connection.catalog_epoch());
    assert!(
        outcome.is_err(),
        "terminal invalidation must not look like a pending initializer"
    );
    assert!(connection.close().await.is_empty());
}

#[tokio::test]
async fn terminal_catalog_failure_is_not_retried_in_the_same_epoch() {
    let fixture = Fixture::new().await;
    let first = fixture.connection();
    let expected = first.discover_catalog().await.unwrap().remove(0);
    assert!(first.close().await.is_empty());
    fixture.peer.fail.store(true, Ordering::SeqCst);
    let connection = fixture.connection();
    assert!(connection.discover_catalog().await.is_err());
    assert_eq!(*fixture.peer.lists.borrow(), 2);
    let result = fixture.execute(connection, expected).await;
    assert!(matches!(
        result.status,
        crate::tools::types::ToolExecutionStatus::Failed { .. }
    ));
    assert_eq!(
        *fixture.peer.lists.borrow(),
        2,
        "first use observes the terminal catalog outcome without launching a second listing"
    );
    assert_eq!(fixture.peer.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn invalidation_reopens_a_failed_generation_catalog_for_fresh_discovery() {
    let fixture = Fixture::new().await;
    let connection = fixture.connection();
    fixture.peer.fail.store(true, Ordering::SeqCst);
    assert!(connection.discover_catalog().await.is_err());
    fixture.peer.fail.store(false, Ordering::SeqCst);
    assert!(connection.discover_catalog().await.is_err());
    assert_eq!(*fixture.peer.lists.borrow(), 1);
    connection
        .reconnect
        .as_ref()
        .unwrap()
        .invalidation
        .lock()
        .advance(&connection.server_id);
    let expected = connection.discover_catalog().await.unwrap().remove(0);
    assert_eq!(*fixture.peer.lists.borrow(), 2);
    assert!(matches!(
        fixture.execute(connection.clone(), expected).await.status,
        crate::tools::types::ToolExecutionStatus::Success
    ));
    assert_eq!(fixture.peer.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn cancelled_validation_does_not_poison_the_same_connection_for_another_caller() {
    let fixture = Fixture::new().await;
    let connection = fixture.connection();
    let expected = connection.discover_catalog().await.unwrap().remove(0);
    connection
        .reconnect
        .as_ref()
        .unwrap()
        .invalidation
        .lock()
        .advance(&connection.server_id);
    fixture.peer.release.send_replace(false);
    let signal = CancellationSignal::new();
    let context = fixture.context(signal.clone());
    let discovery = connection.discover_catalog_owned(Some(&context));
    tokio::pin!(discovery);
    tokio::select! {
        _ = &mut discovery => panic!("live listing held"),
        () = fixture.wait_lists(2) => {},
    }
    signal.cancel();
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(10), discovery)
            .await
            .unwrap()
            .is_err()
    );
    let runtime = connection.published_runtime().unwrap();
    assert_eq!(
        runtime
            .request_ownership
            .as_ref()
            .unwrap()
            .outstanding_requests(),
        0
    );
    fixture.peer.release.send_replace(true);
    let context = fixture.context(CancellationSignal::new());
    assert!(
        connection
            .discover_catalog_owned(Some(&context))
            .await
            .is_ok()
    );
    assert_eq!(*fixture.peer.lists.borrow(), 3);
    assert!(matches!(
        fixture.execute(connection.clone(), expected).await.status,
        crate::tools::types::ToolExecutionStatus::Success
    ));
    assert_eq!(fixture.peer.calls.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.peer.handshakes.load(Ordering::SeqCst), 1);
}
