//! A minimal pre-2026 MCP wire fixture: a server that does **not** implement
//! `server/discover`.
//!
//! # Why this is hand-written
//!
//! Every other fixture in this module is an official-rmcp [`ServerHandler`],
//! and that is the right default. An rmcp 3.1.2 server cannot represent this
//! particular peer, though: its handshake treats *any* non-`initialize`
//! opener as an inline-lifecycle opener and permanently marks the session as
//! requiring self-contained request metadata — even when the handler answers
//! that opener with `METHOD_NOT_FOUND`. A `ServerHandler` therefore cannot
//! impersonate a genuine 2025-era server, and without one there is no
//! regression for rustX's own legacy-path behavior:
//! `legacy_handshake_version()`, the legacy `ClientInfo.protocol_version`,
//! the post-handshake protocol-membership validation, the legacy
//! `tools/list_changed` sink, and stdio settlement.
//!
//! So the handshake here is a hand-written dispatch loop over the
//! newline-delimited JSON-RPC stdio framing. Only the messages this one
//! regression needs are implemented; the `rmcp::model` types are still used
//! for parsing and serialization so the bytes on the wire are exactly the
//! bytes a real MCP peer would produce.
//!
//! **This is a test fixture, not an MCP server implementation, and must not
//! grow into one.**
//!
//! [`ServerHandler`]: rmcp::ServerHandler

use std::io::Write as _;
use std::path::PathBuf;

use rmcp::model::{
    CallToolResult, ClientJsonRpcMessage, ClientNotification, ClientRequest, ContentBlock,
    DiscoverRequestMethod, EmptyResult, ErrorData, Implementation, InitializeResult,
    ListToolsResult, ProtocolVersion, RequestId, ServerCapabilities, ServerJsonRpcMessage,
    ServerNotification, ServerResult, SubscriptionsListenRequestMethod, Tool,
    ToolListChangedNotification,
};
use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader};

/// The environment variable selecting the legacy wire fixture when the test
/// binary is re-executed as its own MCP server.
pub const LEGACY_FIXTURE_MODE_ENV: &str = "RUSTX_M7_LEGACY_MCP_FIXTURE";
/// The environment variable naming the file this fixture appends one line to
/// for every protocol message it observes.
///
/// The journal is the cross-process observation seam: the fixture writes and
/// flushes each entry *before* it answers the message that produced it, and
/// it processes messages strictly in order, so any entry for a message that
/// precedes an answered request is durable by the time the parent observes
/// that answer. No polling and no sleeping is involved.
pub const LEGACY_JOURNAL_ENV: &str = "RUSTX_M7_LEGACY_FIXTURE_JOURNAL";

/// The revision this fixture negotiates over the legacy `initialize`
/// handshake — a representative 2025-era revision.
pub const LEGACY_FIXTURE_REVISION: ProtocolVersion = ProtocolVersion::V_2025_06_18;

/// The journal entry written when `server/discover` is probed.
pub const JOURNAL_DISCOVER: &str = "server/discover";
/// The journal entry prefix written when `initialize` arrives; the suffix is
/// the protocol revision the client requested.
pub const JOURNAL_INITIALIZE_PREFIX: &str = "initialize:";
/// The journal entry written when `notifications/initialized` arrives.
pub const JOURNAL_INITIALIZED: &str = "notifications/initialized";
/// The journal entry written when `tools/list` arrives.
pub const JOURNAL_TOOLS_LIST: &str = "tools/list";
/// The journal entry written when the `mutate` tool is called.
pub const JOURNAL_MUTATE: &str = "tools/call:mutate";
/// The journal entry written if `subscriptions/listen` ever arrives. A
/// correct client never sends it to a pre-2026 peer, so this entry existing
/// at all is the regression failure.
pub const JOURNAL_SUBSCRIBE: &str = "subscriptions/listen";

/// Runs the current test binary as the legacy wire fixture when
/// [`LEGACY_FIXTURE_MODE_ENV`] selects it.
///
/// Returns `true` when it served (so the re-executed test must return
/// immediately), and `false` in the parent process.
pub async fn serve_if_legacy_fixture_mode() -> bool {
    if std::env::var_os(LEGACY_FIXTURE_MODE_ENV).is_none() {
        return false;
    }
    serve(std::env::var_os(LEGACY_JOURNAL_ENV).map(PathBuf::from)).await;
    true
}

/// Reads the journal a fixture run produced.
///
/// # Panics
///
/// Panics when the journal file cannot be read.
#[must_use]
pub fn read_journal(path: &std::path::Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .expect("the legacy fixture journal must exist")
        .lines()
        .map(str::to_owned)
        .collect()
}

/// The fixture catalog: `mutate` is replaced by `new_tool` once called.
fn catalog(changed: bool) -> Vec<Tool> {
    if changed {
        vec![
            super::fixture_tool_named("echo"),
            super::fixture_tool_named("new_tool"),
        ]
    } else {
        vec![
            super::fixture_tool_named("echo"),
            super::fixture_tool_named("mutate"),
        ]
    }
}

/// The hand-written legacy handshake and request loop.
///
/// Message flow, in the order this fixture enforces:
///
/// ```text
/// <- server/discover                 -> error -32601 (connection stays open)
/// <- initialize (legacy revision)    -> InitializeResult(2025-06-18)
/// <- notifications/initialized       (no reply)
/// <- tools/list                      -> [echo, mutate]
/// <- tools/call mutate               -> notifications/tools/list_changed, then success
/// <- tools/list                      -> [echo, new_tool]
/// ```
async fn serve(journal: Option<PathBuf>) {
    let mut input = BufReader::new(tokio::io::stdin());
    let mut output = tokio::io::stdout();
    let mut line = String::new();
    let mut changed = false;
    loop {
        line.clear();
        match input.read_line(&mut line).await {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
        // Unparsable input is ignored, exactly as every MCP SDK's stdio
        // framing does. The test harness itself writes to this process's
        // stdout, so tolerating noise is required, not merely defensive.
        let Ok(message) = serde_json::from_str::<ClientJsonRpcMessage>(line.trim()) else {
            continue;
        };
        match message {
            ClientJsonRpcMessage::Request(request) => {
                let id = request.id.clone();
                for message in handle_request(request.request, id, &mut changed, journal.as_ref()) {
                    send(&mut output, &message).await;
                }
            }
            ClientJsonRpcMessage::Notification(notification) => {
                if matches!(
                    notification.notification,
                    ClientNotification::InitializedNotification(_)
                ) {
                    record(journal.as_ref(), JOURNAL_INITIALIZED);
                }
            }
            ClientJsonRpcMessage::Response(_) | ClientJsonRpcMessage::Error(_) => {}
        }
    }
}

/// Answers one request, returning the messages to write in order.
fn handle_request(
    request: ClientRequest,
    id: RequestId,
    changed: &mut bool,
    journal: Option<&PathBuf>,
) -> Vec<ServerJsonRpcMessage> {
    match request {
        // The whole point of this fixture: a peer that has never heard of the
        // MCP 2026-07-28 inline lifecycle. The connection stays open so the
        // client can fall back on it.
        ClientRequest::DiscoverRequest(_) => {
            record(journal, JOURNAL_DISCOVER);
            vec![ServerJsonRpcMessage::error(
                ErrorData::method_not_found::<DiscoverRequestMethod>(),
                Some(id),
            )]
        }
        ClientRequest::InitializeRequest(request) => {
            // The revision the client offers on the legacy path is rustX's
            // `legacy_handshake_version()`; recording it is what lets the
            // regression assert rustX did not send an inline-only revision.
            record(
                journal,
                &format!(
                    "{JOURNAL_INITIALIZE_PREFIX}{}",
                    request.params.protocol_version
                ),
            );
            let mut result = InitializeResult::new(
                ServerCapabilities::builder()
                    .enable_tools()
                    .enable_tool_list_changed()
                    .build(),
            );
            result.protocol_version = LEGACY_FIXTURE_REVISION;
            result.server_info = Implementation::new("rustx-legacy-fixture", "0.0.0");
            vec![legacy_response(ServerResult::InitializeResult(result), id)]
        }
        ClientRequest::ListToolsRequest(_) => {
            record(journal, JOURNAL_TOOLS_LIST);
            let result = ListToolsResult {
                tools: catalog(*changed),
                ..Default::default()
            };
            vec![legacy_response(ServerResult::ListToolsResult(result), id)]
        }
        ClientRequest::CallToolRequest(request) if request.params.name == "mutate" => {
            record(journal, JOURNAL_MUTATE);
            *changed = true;
            vec![
                // The pre-2026 invalidation wire form: a plain notification,
                // with no subscription behind it.
                ServerJsonRpcMessage::notification(
                    ServerNotification::ToolListChangedNotification(
                        ToolListChangedNotification::default(),
                    ),
                ),
                legacy_response(
                    ServerResult::CallToolResult(CallToolResult::success(vec![
                        ContentBlock::text("fixture changed"),
                    ])),
                    id,
                ),
            ]
        }
        // A client that speaks this revision correctly never sends it; the
        // journal entry is what turns a regression into a visible failure.
        ClientRequest::SubscriptionsListenRequest(_) => {
            record(journal, JOURNAL_SUBSCRIBE);
            vec![ServerJsonRpcMessage::error(
                ErrorData::method_not_found::<SubscriptionsListenRequestMethod>(),
                Some(id),
            )]
        }
        ClientRequest::PingRequest(_) => {
            vec![legacy_response(
                ServerResult::EmptyResult(EmptyResult {}),
                id,
            )]
        }
        _ => vec![ServerJsonRpcMessage::error(
            ErrorData::new(
                rmcp::model::ErrorCode::METHOD_NOT_FOUND,
                "the legacy fixture implements only the pre-2026 regression surface",
                None,
            ),
            Some(id),
        )],
    }
}

/// A response in the pre-2026 wire shape: the SEP-2322 `resultType`
/// discriminator was introduced in 2026-07-28, so a 2025-era peer never
/// sends it.
fn legacy_response(mut result: ServerResult, id: RequestId) -> ServerJsonRpcMessage {
    result.strip_result_type_for_legacy_peer();
    ServerJsonRpcMessage::response(result, id)
}

async fn send(output: &mut tokio::io::Stdout, message: &ServerJsonRpcMessage) {
    let mut line = serde_json::to_vec(message).expect("a fixture message always serializes");
    line.push(b'\n');
    let _ = output.write_all(&line).await;
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
