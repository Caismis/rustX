//! Shared deterministic test infrastructure.
//!
//! # Two provider fixtures, two bounded purposes
//!
//! ```text
//! FixtureServer (this module)        low-level adapter tests: request
//!                                    serialization, stream parsing, error
//!                                    normalization, one-attempt/no-retry.
//!                                    One adapter, no Agent Loop, no runtime.
//!
//! ProviderEmulator (Issue #47)       composed Agent Loop conformance through
//!                                    the real runtime, over a real external
//!                                    provider process with strict ordered
//!                                    scenarios and deterministic gates.
//! ```
//!
//! [`FixtureServer`] is deliberately retained: an adapter translation or
//! stream-normalization test needs one canned body and an attempt counter,
//! and routing it through an external process and a scripted scenario would
//! add a subprocess, a Python toolchain, and a scenario definition to a test
//! that asserts one JSON field. It is **not** the implementation of composed
//! conformance: a test that exercises the Agent Loop, the context engine, the
//! tool runtime, or the capability plane belongs in
//! `tests/issue47_conformance.rs` over [`provider_emulator`].
//!
//! [`FixtureServer`] itself is a small raw-TCP HTTP/1.1 responder that serves
//! fixture bodies, counts attempts, and records request bodies.

#![allow(dead_code)] // every helper is used only by some test binaries

pub mod provider_emulator;

use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::StreamExt;
use rustx::agent::{AgentExecutionResult, state::ExecutionState};
use rustx::durable::ConversationStore;
use rustx::events::types::RuntimeEvent;
use rustx::runtime::identity::AttemptId;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

/// One response to serve, described as a sequence of body chunks. A chunk
/// with a delay lets tests hold a connection open (cancellation tests) or
/// simulate a slow stream. A `header_delay_ms` delays the response status
/// line and headers, letting tests cancel while the client is waiting for
/// the response headers.
pub struct FixtureReply {
    pub status: u16,
    pub reason: &'static str,
    pub headers: Vec<(String, String)>,
    pub header_delay_ms: u64,
    pub chunks: Vec<FixtureChunk>,
}

/// A body chunk written after an optional delay.
pub struct FixtureChunk {
    pub delay_ms: u64,
    pub bytes: Vec<u8>,
}

impl FixtureReply {
    /// A static body reply.
    pub fn body(
        status: u16,
        reason: &'static str,
        content_type: &str,
        body: impl Into<Vec<u8>>,
    ) -> Self {
        Self::chunked(status, reason, content_type, vec![(0, body.into())])
    }

    /// A reply streamed in delayed chunks.
    pub fn chunked(
        status: u16,
        reason: &'static str,
        content_type: &str,
        chunks: Vec<(u64, Vec<u8>)>,
    ) -> Self {
        Self {
            status,
            reason,
            headers: vec![("Content-Type".to_owned(), content_type.to_owned())],
            header_delay_ms: 0,
            chunks: chunks
                .into_iter()
                .map(|(delay_ms, bytes)| FixtureChunk { delay_ms, bytes })
                .collect(),
        }
    }

    /// Delays the response status line and headers by `delay_ms`; the client
    /// observes the connection accepted but no response headers yet.
    #[must_use]
    pub fn with_header_delay(mut self, delay_ms: u64) -> Self {
        self.header_delay_ms = delay_ms;
        self
    }

    /// A provider error body as JSON.
    pub fn json_error(status: u16, reason: &'static str, json: &serde_json::Value) -> Self {
        Self::body(
            status,
            reason,
            "application/json",
            serde_json::to_vec(json).expect("json"),
        )
    }
}

/// A fixture server bound to an ephemeral loopback port.
pub struct FixtureServer {
    pub address: SocketAddr,
    attempts: Arc<AtomicU64>,
    request_bodies: Arc<Mutex<Vec<String>>>,
    handle: tokio::task::JoinHandle<()>,
}

impl FixtureServer {
    /// Starts a server; `responder(attempt, request_head)` decides the reply
    /// for each connection.
    pub async fn start<F>(responder: F) -> Self
    where
        F: Fn(u64, &str) -> FixtureReply + Send + Sync + 'static,
    {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind fixture server");
        let address = listener.local_addr().expect("local address");
        let attempts = Arc::new(AtomicU64::new(0));
        let request_bodies: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let attempts_handle = attempts.clone();
        let bodies_handle = request_bodies.clone();
        let responder = Arc::new(responder);
        let responder_handle = responder.clone();
        let handle = tokio::spawn(async move {
            loop {
                let Ok((socket, _)) = listener.accept().await else {
                    break;
                };
                let attempts = attempts_handle.clone();
                let request_bodies = bodies_handle.clone();
                let responder = responder_handle.clone();
                tokio::spawn(async move {
                    let _ = serve_connection(socket, attempts, request_bodies, &responder).await;
                });
            }
        });
        Self {
            address,
            attempts,
            request_bodies,
            handle,
        }
    }

    /// The number of HTTP attempts the server has observed.
    pub fn attempt_count(&self) -> u64 {
        self.attempts.load(Ordering::SeqCst)
    }

    /// The body of the `index`-th request, in arrival order.
    pub fn request_body(&self, index: usize) -> String {
        self.request_bodies
            .lock()
            .expect("request bodies lock")
            .get(index)
            .cloned()
            .expect("request body exists")
    }

    /// The base URL pointing at this server.
    pub fn url(&self, prefix: &str) -> String {
        format!("http://127.0.0.1:{}{}", self.address.port(), prefix)
    }
}

async fn serve_connection<F>(
    socket: TcpStream,
    attempts: Arc<AtomicU64>,
    request_bodies: Arc<Mutex<Vec<String>>>,
    responder: &Arc<F>,
) -> std::io::Result<()>
where
    F: Fn(u64, &str) -> FixtureReply + Send + Sync,
{
    let attempt = attempts.fetch_add(1, Ordering::SeqCst);
    let (mut read_half, mut write_half) = tokio::io::split(socket);
    let mut head = Vec::new();
    let mut reader = BufReader::new(&mut read_half);
    loop {
        let read = reader.read_until(b'\n', &mut head).await?;
        if read == 0 {
            return Ok(());
        }
        if head.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
    }
    let head_text = String::from_utf8_lossy(&head).into_owned();
    let content_length = parse_content_length(&head_text);
    let mut body = vec![0u8; content_length];
    if content_length > 0 {
        reader.read_exact(&mut body).await?;
    }
    request_bodies
        .lock()
        .expect("request bodies lock")
        .push(String::from_utf8_lossy(&body).into_owned());
    drop(reader);

    let reply = responder(attempt, &head_text);
    if reply.header_delay_ms > 0 {
        tokio::time::sleep(Duration::from_millis(reply.header_delay_ms)).await;
    }
    let mut response = Vec::new();
    response
        .extend_from_slice(format!("HTTP/1.1 {} {}\r\n", reply.status, reply.reason).as_bytes());
    for (name, value) in &reply.headers {
        response.extend_from_slice(format!("{name}: {value}\r\n").as_bytes());
    }
    response.extend_from_slice(b"Connection: close\r\n\r\n");
    write_half.write_all(&response).await?;
    write_half.flush().await?;
    for chunk in reply.chunks {
        if chunk.delay_ms > 0 {
            tokio::time::sleep(Duration::from_millis(chunk.delay_ms)).await;
        }
        // A client that cancelled may have closed the connection; that is a
        // normal outcome, not a test failure.
        if write_half.write_all(&chunk.bytes).await.is_err() {
            return Ok(());
        }
        let _ = write_half.flush().await;
    }
    Ok(())
}

fn parse_content_length(head: &str) -> usize {
    head.lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().unwrap_or(0))
        })
        .unwrap_or(0)
}

/// Collects a canonical event stream into a vector.
pub async fn collect_events(
    adapter: &dyn rustx::model::ModelAdapter,
    request: rustx::model::ModelRequest,
) -> Vec<rustx::model::ModelEvent> {
    let cancellation = rustx::runtime::CancellationSignal::new();
    let mut stream = adapter.stream(request, cancellation);
    let mut events = Vec::new();
    while let Some(event) = stream.next().await {
        events.push(event);
    }
    events
}

/// Collects a canonical event stream with a caller-controlled cancellation.
pub async fn collect_events_with_cancellation(
    adapter: &dyn rustx::model::ModelAdapter,
    request: rustx::model::ModelRequest,
    cancellation: rustx::runtime::CancellationSignal,
) -> Vec<rustx::model::ModelEvent> {
    let mut stream = adapter.stream(request, cancellation);
    let mut events = Vec::new();
    while let Some(event) = stream.next().await {
        events.push(event);
    }
    events
}

/// Loads a fixture file as a static body reply with the SSE content type.
pub fn sse_fixture(dir: &str, name: &str) -> FixtureReply {
    let path = format!("tests/fixtures/m2/{dir}/{name}");
    let body =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("missing fixture {path}: {e}"));
    FixtureReply::body(200, "OK", "text/event-stream", body)
}

/// Loads a JSON error fixture body.
pub fn error_fixture(name: &str) -> FixtureReply {
    let path = format!("tests/fixtures/m2/errors/{name}");
    let body =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("missing fixture {path}: {e}"));
    FixtureReply::body(500, "Internal Server Error", "application/json", body)
}

/// Pretty-prints an event sequence for assertion messages.
pub fn describe_events(events: &[rustx::model::ModelEvent]) -> String {
    events
        .iter()
        .map(|event| serde_json::to_string(event).expect("serialize event"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// A `requestParams` object literal.
///
/// # Panics
///
/// Panics when the value is not a JSON object, which always means the test
/// itself is wrong.
pub fn request_params(value: serde_json::Value) -> rustx::model::RequestParams {
    match value {
        serde_json::Value::Object(map) => map,
        other => panic!("requestParams must be a JSON object, got {other}"),
    }
}

/// The canonical text-only invocation configuration of an adapter test.
///
/// Adapter tests exercise translation, not selection, so the invocation is
/// built directly rather than resolved from a catalog. It carries no request
/// parameters, so any provider field an adapter test observes was produced
/// by translation and not by a configured overlay.
pub fn invocation(
    protocol: rustx::model::ModelProtocol,
    model: &str,
) -> rustx::model::ModelInvocationConfig {
    let chat_reasoning_replay = (protocol == rustx::model::ModelProtocol::OpenAiChatCompletions)
        .then_some(rustx::model::ChatReasoningReplay::Omit);
    rustx::model::ModelInvocationConfig {
        model: model.to_owned(),
        protocol,
        max_output_tokens: 512,
        request_params: rustx::model::RequestParams::new(),
        capabilities: rustx::model::ModelCapabilities::text_only(true, true),
        compat: rustx::model::ModelCompat {
            chat_reasoning_replay,
            ..rustx::model::ModelCompat::default()
        },
    }
}

/// A canonical user-only request for the given protocol.
pub fn simple_request(
    protocol: rustx::model::ModelProtocol,
    model: &str,
    prompt: &str,
) -> rustx::model::ModelRequest {
    use rustx::message::content::TextBlock;
    use rustx::message::types::{MessageBlock, UserContentBlock, UserMessageBlock, UserSource};
    use rustx::runtime::identity::MessageId;
    rustx::model::ModelRequest {
        invocation: invocation(protocol, model),
        messages: vec![MessageBlock::User(UserMessageBlock {
            id: MessageId::new("msg-user-1"),
            content: vec![UserContentBlock::Text(TextBlock {
                text: prompt.to_owned(),
            })],
            source: UserSource::Human,
            kind: rustx::message::types::InboundKind::Message,
            timestamp: None,
        })],
        tools: Vec::new(),
        effective_system_prompt: String::new(),
        continuation: None,
    }
}

/// One canonical tool definition used by adapter tests.
pub fn tool(name: &str, id: &str) -> rustx::tools::types::ToolDefinition {
    use rustx::runtime::identity::ToolId;
    use rustx::tools::types::{
        ToolConcurrencyPolicy, ToolDefinition, ToolExecutionPolicy, ToolOrigin, ToolReplayPolicy,
    };
    ToolDefinition {
        id: ToolId::new(id),
        name: name.to_owned(),
        description: format!("Tool {name}"),
        input_schema: serde_json::json!({"type": "object", "properties": {}}),
        execution_policy: ToolExecutionPolicy::ForegroundOnly,
        concurrency_policy: ToolConcurrencyPolicy::Sequential,
        replay_policy: ToolReplayPolicy::Idempotent,
        origin: ToolOrigin::Mcp {
            server_id: rustx::runtime::identity::McpServerId::new("mcp-test"),
        },
    }
}

/// A process-wide serial for isolating the durable store of each
/// `tool_runtime` fixture.
///
/// Since Issue #63 every canonical commit (assistant/tool/context facts)
/// appends to the conversation's durable Message Ledger, so two loop tests
/// that reuse the same `conversation_id` in one process must not share one
/// durable database file. The serial makes each fixture's storage roots
/// unique without changing the sibling workspace/artifact layout.
static TOOL_RUNTIME_SERIAL: AtomicU64 = AtomicU64::new(0);

/// The isolated sibling storage roots of one `tool_runtime` fixture.
fn tool_runtime_dir(conversation_id: &str) -> std::path::PathBuf {
    let serial = TOOL_RUNTIME_SERIAL.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "rustx-tool-runtime-{conversation_id}-{}-{serial}",
        std::process::id()
    ))
}

/// Test-only full walk over the bounded `RequestHistory` page API.
///
/// Production code must choose an explicit page size; this helper is kept in
/// the test support module so tests can compare complete retained histories
/// without restoring an unbounded production API.
pub fn request_snapshots(
    history: &rustx::runtime::RequestHistory,
) -> Vec<rustx::model::RequestSnapshot> {
    let mut snapshots = Vec::new();
    let mut cursor = None;
    loop {
        let page = history.page(cursor, 32).expect("request snapshot page");
        if page.snapshots.is_empty() {
            break;
        }
        cursor = page.next_sequence;
        snapshots.extend(page.snapshots);
    }
    snapshots
}

/// A test-only audit that loads committed Event Journal facts from the durable
/// authority in bounded pages after an attempt settles. It deliberately does
/// not alter [`AgentExecutionResult`]: production settlement has no complete
/// attempt-local event trace.
pub struct DurableExecutionAudit {
    /// The bounded settlement handoff returned by the Agent Loop.
    pub result: AgentExecutionResult,
    /// The durable Event Journal facts read through fixed-size pages for a
    /// test that is explicitly auditing execution history.
    pub event_history: Vec<RuntimeEvent>,
    /// The durable Request Snapshots read through fixed-size pages for a
    /// test that is explicitly auditing request history.
    snapshot_history: Vec<rustx::model::RequestSnapshot>,
}

impl std::ops::Deref for DurableExecutionAudit {
    type Target = AgentExecutionResult;

    fn deref(&self) -> &Self::Target {
        &self.result
    }
}

/// Reads one attempt's complete Event Journal history through bounded pages.
///
/// This helper is intentionally test-only. Production callers should use the
/// store's page API directly and retain only the page they need.
pub fn read_event_history(
    store: &dyn ConversationStore,
    attempt_id: &AttemptId,
) -> Vec<RuntimeEvent> {
    const PAGE_SIZE: usize = 32;
    let mut cursor = None;
    let mut events = Vec::new();
    loop {
        let page = store
            .read_events(cursor, PAGE_SIZE)
            .expect("durable Event Journal page");
        if page.events.is_empty() {
            break;
        }
        events.extend(
            page.events
                .iter()
                .filter(|envelope| envelope.attempt_id.as_ref() == Some(attempt_id))
                .map(|envelope| envelope.event.clone()),
        );
        cursor = page.next_sequence;
    }
    events
}

/// Reads one conversation's retained Request Snapshots through bounded pages.
pub fn read_request_snapshot_history(
    store: &dyn ConversationStore,
    attempt_id: &AttemptId,
) -> Vec<rustx::model::RequestSnapshot> {
    const PAGE_SIZE: usize = 32;
    let mut cursor = None;
    let mut snapshots = Vec::new();
    loop {
        let page = store
            .read_request_snapshots(cursor, PAGE_SIZE)
            .expect("durable Request Snapshot page");
        if page.snapshots.is_empty() {
            break;
        }
        snapshots.extend(
            page.snapshots
                .into_iter()
                .filter(|snapshot| snapshot.identity.attempt_id == *attempt_id),
        );
        cursor = page.next_sequence;
    }
    snapshots
}

impl DurableExecutionAudit {
    /// Returns the test's explicitly paged durable Request Snapshot audit.
    #[must_use]
    pub fn snapshot_history(&self) -> &[rustx::model::RequestSnapshot] {
        &self.snapshot_history
    }
}

/// Builds the test-only history view from the durable store after settlement.
#[must_use]
pub fn durable_agent_result(
    result: AgentExecutionResult,
    store: &dyn ConversationStore,
) -> DurableExecutionAudit {
    let event_history = read_event_history(store, &result.attempt_id);
    let snapshot_history = read_request_snapshot_history(store, &result.attempt_id);
    DurableExecutionAudit {
        result,
        event_history,
        snapshot_history,
    }
}

/// A conversation tool runtime over a unique temporary workspace.
///
/// Fake tools never touch the workspace, but the durable store now holds the
/// full canonical Message Ledger, so every fixture gets its own storage roots
/// (see [`tool_runtime_dir`]); native-tool tests use isolated temporary
/// workspaces. The artifact root is a sibling of the workspace root, never
/// nested inside it.
#[must_use]
pub fn tool_runtime(conversation_id: &str) -> rustx::tools::runtime::ConversationToolRuntime {
    let dir = tool_runtime_dir(conversation_id);
    let _ = std::fs::create_dir_all(dir.join("workspace"));
    rustx::tools::runtime::ConversationToolRuntime::new(
        rustx::runtime::identity::ConversationId::new(conversation_id),
        dir.join("workspace"),
        dir.join("artifacts"),
    )
    .expect("tool runtime")
}

/// A canonical tool definition with explicit execution/concurrency
/// policies.
#[must_use]
pub fn tool_policies(
    name: &str,
    id: &str,
    execution: rustx::tools::types::ToolExecutionPolicy,
    concurrency: rustx::tools::types::ToolConcurrencyPolicy,
) -> rustx::tools::types::ToolDefinition {
    use rustx::runtime::identity::ToolId;
    use rustx::tools::types::{ToolDefinition, ToolOrigin, ToolReplayPolicy};
    ToolDefinition {
        id: ToolId::new(id),
        name: name.to_owned(),
        description: format!("Tool {name}"),
        input_schema: serde_json::json!({"type": "object", "properties": {}}),
        execution_policy: execution,
        concurrency_policy: concurrency,
        replay_policy: ToolReplayPolicy::Never,
        origin: ToolOrigin::Builtin,
    }
}

/// A conversation tool runtime over a unique temporary workspace with the
/// native tool registry attached.
pub struct NativeFixture {
    /// The temporary directory kept alive for the fixture lifetime.
    #[allow(clippy::used_underscore_binding)]
    _dir: tempfile::TempDir,
    /// The conversation tool runtime.
    pub runtime: rustx::tools::runtime::ConversationToolRuntime,
    /// The registry with every native tool registered.
    pub registry: rustx::tools::executor::ToolRegistry,
    /// The conversation inbound mailbox shared by the runtime and tests.
    pub mailbox: rustx::runtime::inbound::ConversationInboundMailbox,
    /// The full conversation authority used by direct Agent Loop tests.
    /// Tool/runtime code receives only the mailbox capability; the test
    /// harness passes this handle explicitly at the execution boundary.
    pub store: Arc<rustx::durable::SqliteConversationStore>,
}

impl NativeFixture {
    /// The temporary directory backing this fixture.
    #[must_use]
    pub fn dir(&self) -> &tempfile::TempDir {
        &self._dir
    }
}

/// A native tool fixture: isolated temporary workspace and artifact root
/// plus the fully registered native tool plane.
#[must_use]
pub fn native_fixture() -> NativeFixture {
    native_fixture_with_environment(Vec::new())
}

/// A native tool fixture with an explicit authorized tool environment.
#[must_use]
pub fn native_fixture_with_environment(environment: Vec<(String, String)>) -> NativeFixture {
    use rustx::tools::runtime::ConversationRuntimeConfig;
    let dir = tempfile::tempdir().expect("temporary workspace");
    let workspace_root = dir.path().join("workspace");
    std::fs::create_dir_all(&workspace_root).expect("workspace directory");
    let artifacts = dir.path().join("artifacts");
    std::fs::create_dir_all(&artifacts).expect("artifact directory");
    let conversation_id = rustx::runtime::identity::ConversationId::new("conv-m5");
    let store = Arc::new(
        rustx::durable::SqliteConversationStore::open(
            conversation_id.clone(),
            &artifacts.join("conversation.sqlite"),
        )
        .expect("durable store"),
    );
    let environment = rustx::tools::environment::ToolEnvironment::from_authorized(environment)
        .expect("authorized environment");
    let runtime = rustx::tools::runtime::ConversationToolRuntime::from_config(
        conversation_id,
        ConversationRuntimeConfig {
            durable_binding: Some(rustx::durable::ConversationStoreBinding::new(store.clone())),
            environment: Some(environment),
            ..ConversationRuntimeConfig::new(&workspace_root, &artifacts)
        },
    )
    .expect("tool runtime");
    let mut registry = rustx::tools::executor::ToolRegistry::new();
    rustx::tools::native::register_native_tools(
        &mut registry,
        rustx::tools::native::NativeToolResources {
            background: runtime.background().clone(),
        },
        rustx::tools::native::NativeToolPolicies::default(),
    )
    .expect("native tool registration");
    let mailbox = runtime.mailbox();
    NativeFixture {
        _dir: dir,
        runtime,
        registry,
        mailbox,
        store,
    }
}

/// A no-op progress reporter for direct tool invocations.
#[derive(Debug)]
pub struct NoopProgress;

impl rustx::tools::executor::ProgressReporter for NoopProgress {
    fn report(&self, _progress: rustx::tools::types::ToolProgress) {}
}

/// Executes one preflighted native tool call against a fixture with a
/// caller-controlled cancellation signal.
pub async fn run_tool_with_cancellation(
    fixture: &NativeFixture,
    name: &str,
    arguments: serde_json::Value,
    cancellation: rustx::runtime::CancellationSignal,
) -> rustx::tools::types::ToolExecutionResult {
    use rustx::runtime::identity::ToolCallId;
    use rustx::tools::executor::{PreflightOutcome, ToolExecutionContext};
    use rustx::tools::types::ToolCall;
    let definition = fixture
        .registry
        .definitions()
        .into_iter()
        .find(|definition| definition.name == name)
        .expect("tool registered");
    let call = ToolCall {
        id: ToolCallId::new("call-m5"),
        tool_id: definition.id,
        name: name.to_owned(),
        arguments,
    };
    let outcome = fixture.registry.preflight(&call).expect("preflight");
    let PreflightOutcome::Ready(prepared) = outcome else {
        panic!("direct native tool calls preflight as ready");
    };
    let executor = fixture.registry.executor(&prepared.invocation.tool_id);
    let reporter = NoopProgress;
    let context = ToolExecutionContext {
        conversation_id: fixture.runtime.conversation_id(),
        execution_id: None,
        cancellation,
        cancellation_reason: rustx::runtime::types::CancellationReason::UserRequested,
        workspace: fixture.runtime.workspace(),
        progress: &reporter,
        artifacts: fixture.runtime.artifacts(),
        environment: fixture.runtime.environment(),
    };
    executor.execute(prepared.invocation, context).await
}

/// Executes one preflighted native tool call against a fixture.
pub async fn run_tool(
    fixture: &NativeFixture,
    name: &str,
    arguments: serde_json::Value,
) -> rustx::tools::types::ToolExecutionResult {
    use rustx::runtime::identity::ToolCallId;
    use rustx::tools::executor::{PreflightOutcome, ToolExecutionContext};
    use rustx::tools::types::ToolCall;
    let definition = fixture
        .registry
        .definitions()
        .into_iter()
        .find(|definition| definition.name == name)
        .expect("tool registered");
    let call = ToolCall {
        id: ToolCallId::new("call-m5"),
        tool_id: definition.id,
        name: name.to_owned(),
        arguments,
    };
    let outcome = fixture.registry.preflight(&call).expect("preflight");
    let PreflightOutcome::Ready(prepared) = outcome else {
        panic!("direct native tool calls preflight as ready");
    };
    let executor = fixture.registry.executor(&prepared.invocation.tool_id);
    let reporter = NoopProgress;
    let context = ToolExecutionContext {
        conversation_id: fixture.runtime.conversation_id(),
        execution_id: None,
        cancellation: rustx::runtime::CancellationSignal::new(),
        cancellation_reason: rustx::runtime::types::CancellationReason::UserRequested,
        workspace: fixture.runtime.workspace(),
        progress: &reporter,
        artifacts: fixture.runtime.artifacts(),
        environment: fixture.runtime.environment(),
    };
    executor.execute(prepared.invocation, context).await
}

/// One canonical compiled model-facing tool definition used by adapter
/// tests.
pub fn model_tool(name: &str, id: &str) -> rustx::tools::types::ModelToolDefinition {
    use rustx::runtime::identity::ToolId;
    rustx::tools::types::ModelToolDefinition {
        id: ToolId::new(id),
        name: name.to_owned(),
        description: format!("Tool {name}"),
        input_schema: serde_json::json!({"type": "object", "properties": {}}),
    }
}

/// Reconstructs the observable execution-phase sequence from a runtime
/// event trace.
///
/// The fold maps each event to the execution phase it implies and rejects
/// invalid sequences explicitly (a second `AttemptStarted`, an attempt
/// settlement from the wrong phase, or any event after the terminal event).
/// Consecutive equal phases are collapsed, so a text-only trace
/// reconstructs `[Idle, RunningModel, Completed]` and a tool trace
/// `[Idle, RunningModel, WaitingForTool, RunningModel, Completed]`.
pub fn replay_execution_states(events: &[RuntimeEvent]) -> Result<Vec<ExecutionState>, String> {
    let mut phases = vec![ExecutionState::Idle];
    for event in events {
        let current = *phases.last().expect("at least the idle phase");
        if !current.is_active() {
            return Err(format!("event after the terminal phase: {event:?}"));
        }
        let next = match event {
            RuntimeEvent::AttemptStarted { .. } if current == ExecutionState::Idle => {
                ExecutionState::RunningModel
            }
            RuntimeEvent::AttemptStarted { .. } => {
                return Err("attempt started twice".to_owned());
            }
            RuntimeEvent::AttemptCompleted { .. } if current == ExecutionState::RunningModel => {
                ExecutionState::Completed
            }
            RuntimeEvent::AttemptCompleted { .. } => {
                return Err(format!(
                    "attempt completed outside a running-model phase: {event:?}"
                ));
            }
            RuntimeEvent::AttemptFailed { .. } | RuntimeEvent::AttemptCancelled { .. } => {
                ExecutionState::Failed
            }
            RuntimeEvent::ModelRequestStarted { .. } => ExecutionState::RunningModel,
            RuntimeEvent::ToolExecutionStarted { .. }
            | RuntimeEvent::ToolExecutionCompleted { .. }
            | RuntimeEvent::ToolExecutionFailed { .. } => ExecutionState::WaitingForTool,
            _ => current,
        };
        if phases.last() != Some(&next) {
            phases.push(next);
        }
    }
    Ok(phases)
}

/// The attempt capability fixture of one conversation: a coordinator with
/// an empty Skill set over the given immutable tool registry, the
/// conversation's base environment, and a private environment store, with
/// one pinned attempt lease. The temporary directory is kept alive for the
/// fixture lifetime.
pub struct CapabilityFixture {
    /// The temporary directory kept alive for the fixture lifetime.
    #[allow(clippy::used_underscore_binding)]
    _dir: tempfile::TempDir,
    /// The capability coordinator of the conversation.
    pub coordinator: rustx::capabilities::CapabilityCoordinator,
    lease: rustx::capabilities::AttemptCapabilityLease,
}

impl CapabilityFixture {
    /// Moves the pinned attempt capability lease out of the fixture.
    #[must_use]
    pub fn into_lease(self) -> rustx::capabilities::AttemptCapabilityLease {
        self.lease
    }

    /// Moves the lease and coordinator out together when a test must keep the
    /// registered executor handles alive after the attempt settles.
    #[must_use]
    pub fn into_lease_and_coordinator(
        self,
    ) -> (
        rustx::capabilities::AttemptCapabilityLease,
        rustx::capabilities::CapabilityCoordinator,
    ) {
        (self.lease, self.coordinator)
    }
}

/// Builds the attempt capability lease over the given tool registry and
/// conversation tool runtime: an empty Skill set (no discovery, no
/// environment materialization), the base authorized environment, and a
/// private environment store disjoint from the Workspace. The candidate is
/// prepared and committed so the lease pins the established revision.
pub async fn capability_lease(
    tools: rustx::tools::executor::ToolRegistry,
    tool_runtime: &rustx::tools::runtime::ConversationToolRuntime,
) -> CapabilityFixture {
    let dir = tempfile::tempdir().expect("capability temp dir");
    let coordinator = rustx::capabilities::CapabilityCoordinator::new(
        rustx::capabilities::CapabilityCoordinatorConfig {
            conversation_id: tool_runtime.conversation_id().clone(),
            workspace: tool_runtime.workspace().clone(),
            base_tool_registry: std::sync::Arc::new(tools),
            mcp_servers: std::collections::BTreeMap::new(),
            base_environment: tool_runtime.environment().clone(),
            environment_store_root: dir.path().join("skill-env"),
        },
    )
    .expect("capability coordinator");
    let candidate = coordinator
        .prepare_candidate()
        .await
        .expect("candidate preparation");
    coordinator.commit(candidate).expect("candidate commit");
    let lease = coordinator.acquire_attempt_lease();
    CapabilityFixture {
        _dir: dir,
        coordinator,
        lease,
    }
}

// ---------------------------------------------------------------------------
// M6: fake Skill environment backend
// ---------------------------------------------------------------------------

/// One recorded backend call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendCall {
    /// A runtime version resolution.
    ResolvePython,
    /// A runtime version resolution.
    ResolveNode,
    /// A Python environment materialization directly into its final digest directory.
    MaterializePython,
    /// A Node environment materialization into a staging directory.
    MaterializeNode,
}

/// The deterministic fake Skill environment backend: scripted runtime
/// versions, scripted materialization failures, a deterministic
/// materialization gate (for atomic-publication tests), and a complete
/// call record. No test ever touches a public package registry.
#[derive(Clone)]
pub struct FakeSkillEnvironmentBackend {
    inner: std::sync::Arc<FakeBackendInner>,
}

struct FakeBackendInner {
    python_runtime: std::sync::Mutex<String>,
    python_package_manager: std::sync::Mutex<String>,
    node_runtime: std::sync::Mutex<String>,
    node_package_manager: std::sync::Mutex<String>,
    python_failure: std::sync::Mutex<Option<String>>,
    node_failure: std::sync::Mutex<Option<String>>,
    calls: std::sync::Mutex<Vec<BackendCall>>,
    materialize_gate: std::sync::Mutex<Option<MaterializeGate>>,
}

/// The deterministic materialization gate: the fake signals `entered` when
/// materialization begins and blocks until `release` is sent, so a test can
/// observe the store state between materialization and publication.
pub struct MaterializeGate {
    entered_tx: tokio::sync::watch::Sender<bool>,
    entered_rx: tokio::sync::watch::Receiver<bool>,
    release_tx: tokio::sync::watch::Sender<bool>,
    release_rx: tokio::sync::watch::Receiver<bool>,
}

impl MaterializeGate {
    /// Test side: waits until the fake's materialization provably began.
    pub async fn await_entered(&self) {
        let mut rx = self.entered_rx.clone();
        if !*rx.borrow() {
            let _ = rx.changed().await;
        }
    }

    /// Test side: releases the blocked materialization.
    pub fn release(&self) {
        let _ = self.release_tx.send(true);
    }
}

impl FakeSkillEnvironmentBackend {
    /// A deterministic fake backend with fixed scripted runtime versions.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: std::sync::Arc::new(FakeBackendInner {
                python_runtime: std::sync::Mutex::new("Python 3.12.3".to_owned()),
                python_package_manager: std::sync::Mutex::new(
                    "pip 24.0 from /usr/lib/python3/dist-packages/pip (python 3.12)".to_owned(),
                ),
                node_runtime: std::sync::Mutex::new("v22.1.0".to_owned()),
                node_package_manager: std::sync::Mutex::new("10.2.3".to_owned()),
                python_failure: std::sync::Mutex::new(None),
                node_failure: std::sync::Mutex::new(None),
                calls: std::sync::Mutex::new(Vec::new()),
                materialize_gate: std::sync::Mutex::new(None),
            }),
        }
    }

    /// Scripts the Python runtime identity (a runtime-version change
    /// changes the environment digest).
    pub fn set_python_runtime(&self, version: &str) {
        version.clone_into(&mut *self.inner.python_runtime.lock().expect("fake lock"));
    }

    /// Scripts the Node runtime identity.
    pub fn set_node_runtime(&self, version: &str) {
        version.clone_into(&mut *self.inner.node_runtime.lock().expect("fake lock"));
    }

    /// Scripts a Python materialization failure (the next Python
    /// materialization fails with this message).
    pub fn fail_python_materialization(&self, message: &str) {
        *self.inner.python_failure.lock().expect("fake lock") = Some(message.to_owned());
    }

    /// Scripts a Node materialization failure.
    pub fn fail_node_materialization(&self, message: &str) {
        *self.inner.node_failure.lock().expect("fake lock") = Some(message.to_owned());
    }

    /// Installs the deterministic materialization gate.
    pub fn install_materialize_gate(&self) -> MaterializeGate {
        let (entered_tx, entered_rx) = tokio::sync::watch::channel(false);
        let (release_tx, release_rx) = tokio::sync::watch::channel(false);
        let gate = MaterializeGate {
            entered_tx,
            entered_rx,
            release_tx,
            release_rx,
        };
        *self.inner.materialize_gate.lock().expect("fake lock") = Some(gate.clone_for_test());
        gate
    }

    /// The recorded backend calls in order.
    pub fn calls(&self) -> Vec<BackendCall> {
        self.inner.calls.lock().expect("fake lock").clone()
    }

    /// The number of materialization calls of one ecosystem.
    pub fn materialization_count(&self, ecosystem: rustx::skills::Ecosystem) -> usize {
        let call = match ecosystem {
            rustx::skills::Ecosystem::Python => BackendCall::MaterializePython,
            rustx::skills::Ecosystem::Node => BackendCall::MaterializeNode,
        };
        self.calls().into_iter().filter(|c| *c == call).count()
    }

    fn record(&self, call: BackendCall) {
        self.inner.calls.lock().expect("fake lock").push(call);
    }

    async fn gate(&self) {
        let gate = self
            .inner
            .materialize_gate
            .lock()
            .expect("fake lock")
            .clone();
        if let Some(gate) = gate {
            let _ = gate.entered_tx.send(true);
            let mut release = gate.release_rx.clone();
            if !*release.borrow() {
                let _ = release.changed().await;
            }
        }
    }
}

impl Clone for MaterializeGate {
    fn clone(&self) -> Self {
        Self {
            entered_tx: self.entered_tx.clone(),
            entered_rx: self.entered_rx.clone(),
            release_tx: self.release_tx.clone(),
            release_rx: self.release_rx.clone(),
        }
    }
}

impl MaterializeGate {
    fn clone_for_test(&self) -> Self {
        self.clone()
    }
}

impl Default for FakeSkillEnvironmentBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl rustx::skills::SkillEnvironmentBackend for FakeSkillEnvironmentBackend {
    fn resolve_runtime_versions(
        &self,
        ecosystem: rustx::skills::Ecosystem,
    ) -> futures_util::future::BoxFuture<'_, Result<rustx::skills::RuntimeVersions, String>> {
        Box::pin(async move {
            self.record(match ecosystem {
                rustx::skills::Ecosystem::Python => BackendCall::ResolvePython,
                rustx::skills::Ecosystem::Node => BackendCall::ResolveNode,
            });
            match ecosystem {
                rustx::skills::Ecosystem::Python => Ok(rustx::skills::RuntimeVersions {
                    runtime: self.inner.python_runtime.lock().expect("fake lock").clone(),
                    package_manager: self
                        .inner
                        .python_package_manager
                        .lock()
                        .expect("fake lock")
                        .clone(),
                }),
                rustx::skills::Ecosystem::Node => Ok(rustx::skills::RuntimeVersions {
                    runtime: self.inner.node_runtime.lock().expect("fake lock").clone(),
                    package_manager: self
                        .inner
                        .node_package_manager
                        .lock()
                        .expect("fake lock")
                        .clone(),
                }),
            }
        })
    }

    fn materialize_python<'a>(
        &'a self,
        environment_dir: &'a std::path::Path,
        _dependencies: &'a std::collections::BTreeMap<String, String>,
    ) -> futures_util::future::BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            self.record(BackendCall::MaterializePython);
            self.gate().await;
            if let Some(message) = self.inner.python_failure.lock().expect("fake lock").take() {
                return Err(message);
            }
            std::fs::create_dir_all(environment_dir.join("bin"))
                .map_err(|error| error.to_string())?;
            std::fs::write(
                environment_dir.join("bin").join("python"),
                b"#!fake python\n",
            )
            .map_err(|error| error.to_string())?;
            std::fs::write(
                environment_dir.join("bin").join("fake-tool"),
                format!("#!{}\n", environment_dir.join("bin/python").display()),
            )
            .map_err(|error| error.to_string())?;
            std::fs::create_dir_all(environment_dir.join("lib/python3.12/site-packages"))
                .map_err(|error| error.to_string())?;
            Ok(())
        })
    }

    fn materialize_node<'a>(
        &'a self,
        staging: &'a std::path::Path,
        _dependencies: &'a std::collections::BTreeMap<String, String>,
    ) -> futures_util::future::BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            self.record(BackendCall::MaterializeNode);
            self.gate().await;
            if let Some(message) = self.inner.node_failure.lock().expect("fake lock").take() {
                return Err(message);
            }
            std::fs::create_dir_all(staging.join("node_modules/.bin"))
                .map_err(|error| error.to_string())?;
            std::fs::write(
                staging.join("node_modules").join(".bin").join("tool"),
                b"#!fake tool\n",
            )
            .map_err(|error| error.to_string())?;
            Ok(())
        })
    }
}
