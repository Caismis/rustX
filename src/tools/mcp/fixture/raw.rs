//! A hand-written raw-wire MCP fixture for protocol-framing boundary
//! regressions (Issue #174 review): the one peer shape that must emit
//! *protocol-invalid* bytes deliberately.
//!
//! The official-rmcp [`super::FixtureServer`] cannot emit malformed output:
//! its `ServerHandler` serializes only valid messages. The legacy fixture
//! hand-writes the pre-2026 wire for peer-shape coverage but stays on the
//! protocol. This fixture is the deliberate exception that steps off the
//! protocol entirely: it writes a corrupt line at a deterministic point and
//! journals everything the client writes back, so the test can assert what
//! the generic MCP framing does with it.
//!
//! Two corruption kinds are served (selected by [`RAW_CORRUPTION_ENV`]):
//!
//! - `noise` ([`CORRUPTION_NOISE`]): a plain non-JSON line. Per the rmcp
//!   stdio framing this is deliberately ignored (serde `Syntax` category),
//!   and the exchange continues without any protocol-level reply. This is
//!   an implementation characteristic of the transport, not a supported
//!   logging contract.
//! - `invalid` ([`CORRUPTION_INVALID`]): a well-formed JSON line that is
//!   not a valid MCP message (`{"jsonrpc":"2.0","id":1,"method":123}` — a
//!   numeric `method`). This is a confirmed protocol violation (serde
//!   `Data` category): rmcp answers it with a bounded `Invalid Request`
//!   reply to the peer, and rustX's generic observation seam additionally
//!   records it as a rustX protocol fact, fails the in-flight operation,
//!   ends the stream, and poisons the connection generation.
//!
//! The `noise` line is written right after the client's `initialize`
//! request arrives and again before the `tools/call` reply. The `invalid`
//! line is written at exactly one deterministic phase selected by
//! [`RAW_INVALID_PHASE_ENV`]: [`INVALID_PHASE_INITIALIZE`] (before the
//! `initialize` result, while the handshake is pending) or
//! [`INVALID_PHASE_CALL`] (before the `tools/call` reply, mid-exchange).
//!
//! **This is a test fixture, not an MCP server implementation, and must not
//! grow into one.**

use std::io::Write as _;
use std::path::PathBuf;

use rmcp::model::{
    CallToolResult, ClientJsonRpcMessage, ClientRequest, ContentBlock, DiscoverRequestMethod,
    ErrorData, Implementation, InitializeResult, ListToolsResult, ProtocolVersion, RequestId,
    ServerCapabilities, ServerJsonRpcMessage, ServerResult, SubscriptionsListenRequestMethod, Tool,
};
use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader};

/// The environment variable selecting the raw wire fixture when the test
/// binary is re-executed as its own MCP server.
pub const RAW_FIXTURE_MODE_ENV: &str = "RUSTX_M7_RAW_MCP_FIXTURE";
/// The environment variable selecting the corruption kind.
pub const RAW_CORRUPTION_ENV: &str = "RUSTX_M7_RAW_FIXTURE_CORRUPTION";
/// The environment variable selecting the phase at which an `invalid`
/// corruption is emitted.
pub const RAW_INVALID_PHASE_ENV: &str = "RUSTX_M7_RAW_FIXTURE_INVALID_PHASE";
/// The environment variable naming the journal file the fixture appends one
/// line per observed inbound message to (the cross-process observation
/// seam).
pub const RAW_JOURNAL_ENV: &str = "RUSTX_M7_RAW_FIXTURE_JOURNAL";
/// The `RAW_CORRUPTION_ENV` value writing a plain non-JSON line.
pub const CORRUPTION_NOISE: &str = "noise";
/// The `RAW_CORRUPTION_ENV` value writing a well-formed-JSON wrong-shape
/// message.
pub const CORRUPTION_INVALID: &str = "invalid";
/// The `RAW_INVALID_PHASE_ENV` value emitting the invalid message before
/// the `initialize` result, while the handshake is pending.
pub const INVALID_PHASE_INITIALIZE: &str = "initialize";
/// The `RAW_INVALID_PHASE_ENV` value emitting the invalid message before
/// the `tools/call` reply, mid-exchange.
pub const INVALID_PHASE_CALL: &str = "call";

/// The journal entry prefix written for every inbound line the fixture
/// observes; the suffix is the truncated raw line.
pub const JOURNAL_INBOUND_PREFIX: &str = "in:";
/// The journal entry written when the fixture observes the client's bounded
/// protocol-error reply (`Invalid Request`) to its corrupt line.
pub const JOURNAL_CLIENT_PROTOCOL_ERROR: &str = "client-protocol-error";
/// The journal entry written when `tools/list` is answered.
pub const JOURNAL_TOOLS_LIST: &str = "tools/list";
/// The journal entry written when the `echo` tool is answered.
pub const JOURNAL_ECHO: &str = "tools/call:echo";

/// Runs the current test binary as the raw wire fixture when
/// [`RAW_FIXTURE_MODE_ENV`] selects it.
///
/// Returns `true` when it served (so the re-executed test must return
/// immediately), and `false` in the parent process.
pub async fn serve_if_raw_fixture_mode() -> bool {
    if std::env::var_os(RAW_FIXTURE_MODE_ENV).is_none() {
        return false;
    }
    let journal = std::env::var_os(RAW_JOURNAL_ENV).map(PathBuf::from);
    serve(journal.as_ref()).await;
    true
}

/// One line the fixture writes: either a raw byte line (the corruption) or
/// a serialized protocol message. The message variant is boxed because the
/// enum is held per output line and the message is by far the larger shape.
enum WireOutput {
    Raw(String),
    Message(Box<ServerJsonRpcMessage>),
}

/// The fixture catalog: one `echo` tool.
fn catalog() -> Vec<Tool> {
    vec![super::fixture_tool_named("echo")]
}

/// The hand-written wire loop. Message flow:
///
/// ```text
/// <- server/discover                 -> error -32601 (connection stays open)
/// <- initialize (legacy revision)    -> [corrupt line], InitializeResult
/// <- notifications/initialized       (no reply)
/// <- tools/list                      -> [echo]
/// <- tools/call echo                 -> [corrupt line], success
/// ```
///
/// The `invalid` corrupt line appears at exactly one of the two bracketed
/// points, selected by [`RAW_INVALID_PHASE_ENV`]; the `noise` line appears
/// at both.
async fn serve(journal: Option<&PathBuf>) {
    let corruption =
        std::env::var(RAW_CORRUPTION_ENV).unwrap_or_else(|_| CORRUPTION_NOISE.to_owned());
    let invalid_phase = std::env::var(RAW_INVALID_PHASE_ENV)
        .unwrap_or_else(|_| INVALID_PHASE_INITIALIZE.to_owned());
    let mut input = BufReader::new(tokio::io::stdin());
    let mut output = tokio::io::stdout();
    let mut line = String::new();
    loop {
        line.clear();
        match input.read_line(&mut line).await {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
        let trimmed = line.trim_end();
        // Journal every inbound line, so the parent can assert exactly what
        // the client wrote back — including any protocol-error reply.
        if !trimmed.is_empty() {
            let mut entry = trimmed.to_owned();
            entry.truncate(300);
            record(journal, &format!("{JOURNAL_INBOUND_PREFIX}{entry}"));
            if entry.contains("\"error\"") {
                record(journal, JOURNAL_CLIENT_PROTOCOL_ERROR);
            }
        }
        let Ok(message) = serde_json::from_str::<ClientJsonRpcMessage>(trimmed) else {
            continue;
        };
        match message {
            ClientJsonRpcMessage::Request(request) => {
                let id = request.id.clone();
                for output_line in
                    handle_request(request.request, id, journal, &corruption, &invalid_phase)
                {
                    write_line(&mut output, output_line).await;
                }
            }
            ClientJsonRpcMessage::Notification(_)
            | ClientJsonRpcMessage::Response(_)
            | ClientJsonRpcMessage::Error(_) => {}
        }
    }
}

/// Answers one request. The `invalid` corrupt line is emitted at exactly
/// one deterministic phase: before the `initialize` result (while the
/// handshake response is pending) or before the `tools/call` reply
/// (mid-exchange). `noise` corrupts both phases.
fn handle_request(
    request: ClientRequest,
    id: RequestId,
    journal: Option<&PathBuf>,
    corruption: &str,
    invalid_phase: &str,
) -> Vec<WireOutput> {
    match request {
        ClientRequest::DiscoverRequest(_) => {
            // The whole point of this fixture is the corruption, not the
            // lifecycle: answer the modern-lifecycle probe with the plain
            // method-not-found error so the client falls back to the legacy
            // `initialize` handshake this fixture implements.
            vec![WireOutput::Message(Box::new(ServerJsonRpcMessage::error(
                ErrorData::method_not_found::<DiscoverRequestMethod>(),
                Some(id),
            )))]
        }
        ClientRequest::InitializeRequest(_) => {
            let mut result = InitializeResult::new(
                ServerCapabilities::builder()
                    .enable_tools()
                    .enable_tool_list_changed()
                    .build(),
            );
            result.protocol_version = ProtocolVersion::V_2025_06_18;
            result.server_info = Implementation::new("rustx-raw-fixture", "0.0.0");
            let mut outputs = Vec::new();
            if corruption == CORRUPTION_NOISE || invalid_phase == INVALID_PHASE_INITIALIZE {
                outputs.push(WireOutput::Raw(corrupt_line(corruption)));
            }
            outputs.push(WireOutput::Message(Box::new(legacy_response(
                ServerResult::InitializeResult(result),
                id,
            ))));
            outputs
        }
        ClientRequest::ListToolsRequest(_) => {
            record(journal, JOURNAL_TOOLS_LIST);
            let result = ListToolsResult {
                tools: catalog(),
                ..Default::default()
            };
            vec![WireOutput::Message(Box::new(legacy_response(
                ServerResult::ListToolsResult(result),
                id,
            )))]
        }
        ClientRequest::CallToolRequest(request) if request.params.name == "echo" => {
            record(journal, JOURNAL_ECHO);
            let mut outputs = Vec::new();
            if corruption == CORRUPTION_NOISE || invalid_phase == INVALID_PHASE_CALL {
                outputs.push(WireOutput::Raw(corrupt_line(corruption)));
            }
            outputs.push(WireOutput::Message(Box::new(legacy_response(
                ServerResult::CallToolResult(CallToolResult::success(vec![ContentBlock::text(
                    "echo",
                )])),
                id,
            ))));
            outputs
        }
        ClientRequest::PingRequest(_) => {
            vec![WireOutput::Message(Box::new(legacy_response(
                ServerResult::EmptyResult(rmcp::model::EmptyResult {}),
                id,
            )))]
        }
        // A client that speaks this revision correctly never sends it.
        ClientRequest::SubscriptionsListenRequest(_) => {
            vec![WireOutput::Message(Box::new(ServerJsonRpcMessage::error(
                ErrorData::method_not_found::<SubscriptionsListenRequestMethod>(),
                Some(id),
            )))]
        }
        _ => vec![WireOutput::Message(Box::new(ServerJsonRpcMessage::error(
            ErrorData::new(
                rmcp::model::ErrorCode::METHOD_NOT_FOUND,
                "the raw fixture implements only the framing regression surface",
                None,
            ),
            Some(id),
        )))],
    }
}

/// The corrupt line to write at the selected phase: plain noise or a
/// well-formed JSON line that is not a valid MCP message (a numeric
/// `method` field).
fn corrupt_line(corruption: &str) -> String {
    match corruption {
        CORRUPTION_INVALID => "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":123}".to_owned(),
        _ => "plain non-protocol noise on the wire".to_owned(),
    }
}

/// A response in the pre-2026 wire shape: the SEP-2322 `resultType`
/// discriminator was introduced in 2026-07-28, so a 2025-era peer never
/// sends it.
fn legacy_response(mut result: ServerResult, id: RequestId) -> ServerJsonRpcMessage {
    result.strip_result_type_for_legacy_peer();
    ServerJsonRpcMessage::response(result, id)
}

async fn write_line(output: &mut tokio::io::Stdout, line: WireOutput) {
    let mut bytes = match line {
        WireOutput::Raw(raw) => raw.into_bytes(),
        WireOutput::Message(message) => {
            serde_json::to_vec(message.as_ref()).expect("a fixture message always serializes")
        }
    };
    bytes.push(b'\n');
    let _ = output.write_all(&bytes).await;
    let _ = output.flush().await;
}

/// Appends one durable journal entry.
fn record(journal: Option<&PathBuf>, entry: &str) {
    let Some(path) = journal else {
        return;
    };
    let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    else {
        return;
    };
    let _ = writeln!(file, "{entry}");
    let _ = file.flush();
}

/// Reads the journal a fixture run produced.
///
/// # Panics
///
/// Panics when the journal file cannot be read.
#[must_use]
pub fn read_journal(path: &std::path::Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .expect("the raw fixture journal must exist")
        .lines()
        .map(str::to_owned)
        .collect()
}
