//! Shared deterministic test infrastructure: a minimal local HTTP fixture
//! server.
//!
//! Deterministic adapter tests must be network-free with respect to real
//! providers. A test web framework is deliberately avoided; this module is a
//! small raw-TCP HTTP/1.1 responder that serves fixture bodies, counts
//! attempts, and records request bodies so tests can assert exact HTTP
//! behavior (one attempt, no retry, correct serialization).

#![allow(dead_code)] // every helper is used only by some test binaries

pub mod context;
pub mod fake;

use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::StreamExt;
use rustx::agent::state::ExecutionState;
use rustx::events::types::RuntimeEvent;
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

/// A canonical user-only request for the given protocol.
pub fn simple_request(
    protocol: rustx::model::ModelProtocol,
    model: &str,
    prompt: &str,
) -> rustx::model::ModelRequest {
    use rustx::message::content::TextBlock;
    use rustx::message::types::{MessageBlock, UserContentBlock, UserMessageBlock, UserSource};
    use rustx::model::ReasoningEffort;
    use rustx::runtime::identity::MessageId;
    rustx::model::ModelRequest {
        model: model.to_owned(),
        protocol,
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
        agent_status: None,
        reasoning: ReasoningEffort::Medium,
        max_output_tokens: 512,
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

/// A conversation tool runtime over a unique temporary workspace.
///
/// Fake tools never touch the workspace, so M3/M4 tests share one runtime
/// per conversation id; native-tool tests use isolated temporary workspaces.
#[must_use]
pub fn tool_runtime(conversation_id: &str) -> rustx::tools::runtime::ConversationToolRuntime {
    let dir = std::env::temp_dir().join(format!(
        "rustx-tool-runtime-{conversation_id}-{}",
        std::process::id()
    ));
    let _ = std::fs::create_dir_all(&dir);
    rustx::tools::runtime::ConversationToolRuntime::new(
        rustx::runtime::identity::ConversationId::new(conversation_id),
        &dir,
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
    let dir = tempfile::tempdir().expect("temporary workspace");
    let workspace_root = dir.path().join("workspace");
    std::fs::create_dir_all(&workspace_root).expect("workspace directory");
    let artifacts = dir.path().join("artifacts");
    let conversation_id = rustx::runtime::identity::ConversationId::new("conv-m5");
    let mailbox = rustx::runtime::inbound::ConversationInboundMailbox::new(conversation_id.clone());
    let environment = rustx::tools::environment::ToolEnvironment::from_authorized(environment)
        .expect("authorized environment");
    let runtime = rustx::tools::runtime::ConversationToolRuntime::new(
        conversation_id,
        &workspace_root,
        &artifacts,
    )
    .expect("tool runtime")
    .with_mailbox(mailbox.clone())
    .with_environment(environment);
    let mut registry = rustx::tools::executor::ToolRegistry::new();
    rustx::tools::native::register_native_tools(
        &mut registry,
        rustx::tools::native::NativeToolResources {
            background: runtime.background().clone(),
        },
    )
    .expect("native tool registration");
    NativeFixture {
        _dir: dir,
        runtime,
        registry,
        mailbox,
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
