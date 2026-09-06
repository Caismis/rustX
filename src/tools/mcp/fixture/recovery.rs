//! The scripted MCP fixture server for Issue #205's liveness, cancellation,
//! transport-loss, and reconnection regressions.
//!
//! The official-rmcp [`super::FixtureServer`] answers every request; this
//! fixture exists because the #205 contract is about servers that
//! deliberately do *not*: one that never answers a dispatched `tools/call`,
//! one that dies with the request in flight, and one that answers only when
//! the test releases it.
//!
//! # The two cross-process observation seams
//!
//! A self-spawned stdio fixture's state lives in another process, so the
//! parent proves ordering through two seams, never through timing:
//!
//! - **the journal file.** Every generation appends one line per lifecycle
//!   event and per accepted `tools/call`, before it acts on it. Waiting for
//!   a `call:` line is how the parent proves a dispatched request reached
//!   the server — strictly stronger than the effect frontier rustX
//!   classifies against (`send_cancellable_request` returning `Ok`), so it
//!   is a sound gate for every post-frontier claim. Counting those lines is
//!   how "the ambiguous invocation was received at most once" is proven
//!   across a reconnection, since two generations are two processes.
//! - **the released progress seam.** [`TOOL_HANG`] emits a
//!   `notifications/progress` for pulse `i` only once the parent creates
//!   that pulse's release marker, which is always long after the client
//!   subscribed to the request's progress token. Remote liveness evidence
//!   therefore appears exactly when the parent asked for it, with no
//!   subscription race — the parent observes each pulse through the ordinary
//!   [`crate::tools::executor::ProgressReporter`].
//!
//! **This is a test fixture, not an MCP server implementation, and must not
//! grow into one.**

use std::io::Write as _;
use std::path::{Path, PathBuf};

use rmcp::model::{
    CallToolRequest, CallToolResult, ClientJsonRpcMessage, ClientRequest, ContentBlock,
    DiscoverRequestMethod, ErrorData, Implementation, InitializeResult, ListToolsResult,
    ProgressNotificationParam, ProgressToken, ProtocolVersion, RequestId, RequestMetaObject,
    RequestParamsMeta, ServerCapabilities, ServerJsonRpcMessage, ServerNotification, ServerResult,
    Tool,
};
use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader};

/// Selects the recovery fixture when the test binary is re-executed as its
/// own MCP server.
pub const RECOVERY_FIXTURE_MODE_ENV: &str = "RUSTX_MCP_RECOVERY_FIXTURE";
/// Names the append-only journal file shared by every generation of one
/// fixture server identity.
pub const RECOVERY_JOURNAL_ENV: &str = "RUSTX_MCP_RECOVERY_JOURNAL";
/// Names the counter file that gives each spawned generation its number.
pub const RECOVERY_GENERATION_FILE_ENV: &str = "RUSTX_MCP_RECOVERY_GENERATION_FILE";
/// Names the directory in which the parent creates tool release markers.
pub const RECOVERY_RELEASE_DIR_ENV: &str = "RUSTX_MCP_RECOVERY_RELEASE_DIR";
/// The comma-separated generation numbers whose `tools/call` handling exits
/// the process instead of answering (transport loss after dispatch).
pub const RECOVERY_DIE_GENERATIONS_ENV: &str = "RUSTX_MCP_RECOVERY_DIE_GENERATIONS";
/// The comma-separated generation numbers that exit immediately after
/// claiming their number, before the handshake, so the client's bounded
/// reconnect attempt provably fails.
pub const RECOVERY_REFUSE_GENERATIONS_ENV: &str = "RUSTX_MCP_RECOVERY_REFUSE_GENERATIONS";
/// The comma-separated generation numbers that emit one structurally
/// invalid MCP/JSON-RPC line instead of answering a `tools/call`: a
/// confirmed protocol violation that must fail closed.
pub const RECOVERY_CORRUPT_GENERATIONS_ENV: &str = "RUSTX_MCP_RECOVERY_CORRUPT_GENERATIONS";
/// The comma-separated generation numbers that publish the extra tool in
/// their catalog, so a successful refresh has an observable catalog change.
pub const RECOVERY_EXTRA_TOOL_GENERATIONS_ENV: &str = "RUSTX_MCP_RECOVERY_EXTRA_TOOL_GENERATIONS";
/// How many additional gated progress notifications the `hang` tool emits
/// after its dispatch notification. Pulse `i` is emitted once the parent
/// creates the `hang.<i>` release marker, so the parent controls exactly
/// when remote liveness evidence appears.
pub const RECOVERY_HANG_PULSES_ENV: &str = "RUSTX_MCP_RECOVERY_HANG_PULSES";

/// The journal prefix of one established generation.
pub const JOURNAL_GENERATION_PREFIX: &str = "generation:";
/// The journal prefix of one accepted `tools/call`, followed by the tool
/// name. Counting these lines proves how many times the server was actually
/// asked to execute a tool.
pub const JOURNAL_CALL_PREFIX: &str = "call:";
/// The journal line written when a generation exits with a request in
/// flight.
pub const JOURNAL_DIED: &str = "died-after-dispatch";
/// The journal prefix of one observed `notifications/cancelled`.
pub const JOURNAL_CANCELLED_PREFIX: &str = "cancelled:";
/// The journal prefix of one generation that refused to handshake at all.
pub const JOURNAL_REFUSED_PREFIX: &str = "refused:";
/// The journal line written when a generation emits its corrupt line.
pub const JOURNAL_CORRUPTED: &str = "emitted-invalid-message";

/// The tool that answers immediately.
pub const TOOL_ECHO: &str = "echo";
/// The tool that emits its dispatch progress notification and then never
/// answers, keeping the transport open and healthy.
pub const TOOL_HANG: &str = "hang";
/// The tool that emits exactly one progress notification and then answers
/// immediately, with no gate between the two.
///
/// It is the deterministic regression for "remote liveness evidence that
/// arrives just before the correlated response is never discarded": the
/// notification and the response leave the server in that order on one
/// ordered byte stream, and the client subscribes to the request's progress
/// token only after dispatching it, so both the pre-subscription window and
/// the biased response-versus-progress arbitration are exercised at once.
pub const TOOL_ANNOUNCE: &str = "announce";
/// The tool published only by the generations named in
/// [`RECOVERY_EXTRA_TOOL_GENERATIONS_ENV`]: the observable catalog change of
/// a successful capability refresh.
pub const TOOL_EXTRA: &str = "extra";
// The fixture deliberately has no tool that blocks its read loop: a server
// that stops reading could not observe `notifications/cancelled`, which is
// exactly the protocol fact several regressions must prove reached it.

/// The published catalog of one generation.
#[must_use]
pub fn catalog(with_extra: bool) -> Vec<Tool> {
    let mut tools = vec![
        super::fixture_tool_named(TOOL_ANNOUNCE),
        super::fixture_tool_named(TOOL_ECHO),
        super::fixture_tool_named(TOOL_HANG),
    ];
    if with_extra {
        tools.push(super::fixture_tool_named(TOOL_EXTRA));
    }
    tools
}

/// Runs the current test binary as the recovery fixture when
/// [`RECOVERY_FIXTURE_MODE_ENV`] selects it.
///
/// Returns `true` when it served, so the re-executed test returns
/// immediately.
pub async fn serve_if_recovery_fixture_mode() -> bool {
    if std::env::var_os(RECOVERY_FIXTURE_MODE_ENV).is_none() {
        return false;
    }
    serve().await;
    true
}

/// The scripted per-generation behaviour of one recovery fixture server
/// identity.
///
/// Generations are numbered from 1 in spawn order, so a parent test names
/// exactly which transport generation misbehaves and how.
#[derive(Debug, Clone, Default)]
pub struct RecoveryScript {
    /// Generations that exit with the request in flight, after journaling
    /// the accepted call and emitting its dispatch notification.
    pub die_generations: Vec<u64>,
    /// Generations that exit before the handshake, so a bounded reconnect
    /// attempt provably fails.
    pub refuse_generations: Vec<u64>,
    /// Generations that answer a `tools/call` with one structurally invalid
    /// MCP/JSON-RPC line.
    pub corrupt_generations: Vec<u64>,
    /// Generations that publish [`TOOL_EXTRA`] in their catalog.
    pub extra_tool_generations: Vec<u64>,
    /// How many gated progress pulses [`TOOL_HANG`] emits.
    pub hang_pulses: u32,
}

fn joined(values: &[u64]) -> String {
    values
        .iter()
        .map(u64::to_string)
        .collect::<Vec<_>>()
        .join(",")
}

/// The cross-process control files of one recovery fixture server identity.
///
/// The journal is the parent's observation seam and the generation counter
/// is what gives every spawned server process its number, so both are shared
/// by every generation of the same server identity.
#[derive(Debug, Clone)]
pub struct RecoveryControl {
    /// The append-only journal of lifecycle and `tools/call` facts.
    pub journal: PathBuf,
    /// The generation counter file.
    pub generation_file: PathBuf,
    /// The directory in which the parent creates release markers.
    pub release_dir: PathBuf,
}

impl RecoveryControl {
    /// Creates the control files under `root`.
    ///
    /// # Panics
    ///
    /// Panics if the release directory cannot be created.
    #[must_use]
    pub fn new(root: &Path) -> Self {
        let release_dir = root.join("mcp-recovery-release");
        std::fs::create_dir_all(&release_dir).expect("release directory");
        Self {
            journal: root.join("mcp-recovery.journal"),
            generation_file: root.join("mcp-recovery.generation"),
            release_dir,
        }
    }

    /// The environment a parent test hands to one recovery fixture binding.
    #[must_use]
    pub fn environment(
        &self,
        script: &RecoveryScript,
    ) -> std::collections::BTreeMap<String, String> {
        std::collections::BTreeMap::from([
            (RECOVERY_FIXTURE_MODE_ENV.to_owned(), "1".to_owned()),
            (
                RECOVERY_JOURNAL_ENV.to_owned(),
                self.journal.display().to_string(),
            ),
            (
                RECOVERY_GENERATION_FILE_ENV.to_owned(),
                self.generation_file.display().to_string(),
            ),
            (
                RECOVERY_RELEASE_DIR_ENV.to_owned(),
                self.release_dir.display().to_string(),
            ),
            (
                RECOVERY_DIE_GENERATIONS_ENV.to_owned(),
                joined(&script.die_generations),
            ),
            (
                RECOVERY_REFUSE_GENERATIONS_ENV.to_owned(),
                joined(&script.refuse_generations),
            ),
            (
                RECOVERY_CORRUPT_GENERATIONS_ENV.to_owned(),
                joined(&script.corrupt_generations),
            ),
            (
                RECOVERY_EXTRA_TOOL_GENERATIONS_ENV.to_owned(),
                joined(&script.extra_tool_generations),
            ),
            (
                RECOVERY_HANG_PULSES_ENV.to_owned(),
                script.hang_pulses.to_string(),
            ),
        ])
    }

    /// Every journal line written so far, oldest first.
    #[must_use]
    pub fn journal_entries(&self) -> Vec<String> {
        std::fs::read_to_string(&self.journal)
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    /// Counts the accepted `tools/call` requests for one tool across every
    /// generation of the server.
    #[must_use]
    pub fn accepted_calls(&self, tool: &str) -> usize {
        let entry = format!("{JOURNAL_CALL_PREFIX}{tool}");
        self.journal_entries()
            .into_iter()
            .filter(|line| *line == entry)
            .count()
    }

    /// How many server generations were ever spawned for this identity.
    #[must_use]
    pub fn established_generations(&self) -> usize {
        self.journal_entries()
            .into_iter()
            .filter(|line| line.starts_with(JOURNAL_GENERATION_PREFIX))
            .count()
    }

    /// Releases the `hang` tool's `pulse`-th progress notification.
    ///
    /// # Panics
    ///
    /// Panics if the marker cannot be written.
    pub fn release_hang_pulse(&self, pulse: u32) {
        std::fs::write(
            self.release_dir
                .join(format!("{TOOL_HANG}.{pulse}.release")),
            "go",
        )
        .expect("release marker write");
    }
}

/// The progress token of one `tools/call`.
///
/// rmcp lifts the wire's `params._meta` into the request's typed extensions,
/// so the token is read from there rather than from the business params.
fn request_progress_token(call: &CallToolRequest) -> Option<ProgressToken> {
    if let Some(token) = call
        .extensions
        .get::<RequestMetaObject>()
        .and_then(RequestMetaObject::get_progress_token)
    {
        return Some(token);
    }
    call.params.progress_token()
}

fn record(journal: Option<&PathBuf>, entry: &str) {
    let Some(journal) = journal else {
        return;
    };
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(journal)
    {
        let _ = writeln!(file, "{entry}");
        let _ = file.flush();
    }
}

/// Claims this process's generation number from the shared counter file.
///
/// The counter is bumped once per spawned server process, so the parent can
/// name a specific transport generation's behavior deterministically.
fn claim_generation() -> u64 {
    let Some(path) = std::env::var_os(RECOVERY_GENERATION_FILE_ENV).map(PathBuf::from) else {
        return 1;
    };
    let previous = std::fs::read_to_string(&path)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or(0);
    let generation = previous + 1;
    let _ = std::fs::write(&path, generation.to_string());
    generation
}

async fn write_message(output: &mut tokio::io::Stdout, mut result: ServerResult, id: RequestId) {
    result.strip_result_type_for_legacy_peer();
    let mut bytes = serde_json::to_vec(&ServerJsonRpcMessage::response(result, id))
        .expect("a fixture message always serializes");
    bytes.push(b'\n');
    let _ = output.write_all(&bytes).await;
    let _ = output.flush().await;
}

async fn write_error(output: &mut tokio::io::Stdout, error: ErrorData, id: RequestId) {
    let mut bytes = serde_json::to_vec(&ServerJsonRpcMessage::error(error, Some(id)))
        .expect("a fixture error always serializes");
    bytes.push(b'\n');
    let _ = output.write_all(&bytes).await;
    let _ = output.flush().await;
}

async fn write_progress(output: &mut tokio::io::Stdout, params: ProgressNotificationParam) {
    let notification = ServerJsonRpcMessage::notification(
        ServerNotification::ProgressNotification(rmcp::model::Notification::new(params)),
    );
    let mut bytes =
        serde_json::to_vec(&notification).expect("a fixture notification always serializes");
    bytes.push(b'\n');
    let _ = output.write_all(&bytes).await;
    let _ = output.flush().await;
}

/// Waits for the parent's release marker.
///
/// The wait is a rendezvous on a file the parent creates, never a timed
/// guess: the poll interval only bounds how quickly the rendezvous is
/// noticed, and the ordering it proves ("the parent released it") is
/// established by the marker's existence alone.
async fn await_release(release_dir: Option<&PathBuf>, tool: &str) {
    let Some(release_dir) = release_dir else {
        return;
    };
    let marker = release_dir.join(format!("{tool}.release"));
    while !marker.exists() {
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
}

/// The hand-written wire loop. Message flow:
///
/// ```text
/// <- server/discover            -> error -32601 (legacy fallback)
/// <- initialize                 -> InitializeResult
/// <- notifications/initialized  (no reply)
/// <- tools/list                 -> [echo, hang] (+ extra for selected generations)
/// <- tools/call <tool>          -> progress notification, then per-tool behavior
/// <- notifications/cancelled    (journaled, never answered — MCP defines no ack)
/// ```
async fn serve() {
    let journal = std::env::var_os(RECOVERY_JOURNAL_ENV).map(PathBuf::from);
    let release_dir = std::env::var_os(RECOVERY_RELEASE_DIR_ENV).map(PathBuf::from);
    let generation = Generation::from_env();
    record(
        journal.as_ref(),
        &format!("{JOURNAL_GENERATION_PREFIX}{}", generation.number),
    );
    if generation.refuses {
        // The replacement transport never completes a handshake, so the
        // client's one bounded reconnect attempt fails before any request
        // can exist.
        record(
            journal.as_ref(),
            &format!("{JOURNAL_REFUSED_PREFIX}{}", generation.number),
        );
        return;
    }

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
        if trimmed.is_empty() {
            continue;
        }
        let Ok(message) = serde_json::from_str::<ClientJsonRpcMessage>(trimmed) else {
            continue;
        };
        let request = match message {
            ClientJsonRpcMessage::Request(request) => request,
            ClientJsonRpcMessage::Notification(notification) => {
                let raw = serde_json::to_string(&notification).unwrap_or_default();
                if raw.contains("notifications/cancelled") {
                    record(
                        journal.as_ref(),
                        &format!("{JOURNAL_CANCELLED_PREFIX}{}", generation.number),
                    );
                }
                continue;
            }
            ClientJsonRpcMessage::Response(_) | ClientJsonRpcMessage::Error(_) => continue,
        };
        let id = request.id.clone();
        match request.request {
            ClientRequest::CallToolRequest(call) => {
                serve_call(
                    &mut output,
                    call,
                    id,
                    &generation,
                    journal.as_ref(),
                    release_dir.as_ref(),
                )
                .await;
            }
            other => serve_lifecycle(&mut output, other, id, &generation).await,
        }
    }
}

/// Answers everything that is not a `tools/call`: the lifecycle handshake,
/// the catalog, and the deliberate refusal of anything else.
async fn serve_lifecycle(
    output: &mut tokio::io::Stdout,
    request: ClientRequest,
    id: RequestId,
    generation: &Generation,
) {
    match request {
        ClientRequest::DiscoverRequest(_) => {
            write_error(
                output,
                ErrorData::method_not_found::<DiscoverRequestMethod>(),
                id,
            )
            .await;
        }
        ClientRequest::InitializeRequest(_) => {
            let mut result =
                InitializeResult::new(ServerCapabilities::builder().enable_tools().build());
            result.protocol_version = ProtocolVersion::V_2025_06_18;
            result.server_info = Implementation::new("rustx-recovery-fixture", "0.0.0");
            write_message(output, ServerResult::InitializeResult(result), id).await;
        }
        ClientRequest::ListToolsRequest(_) => {
            let result = ListToolsResult {
                tools: catalog(generation.publishes_extra_tool),
                ..Default::default()
            };
            write_message(output, ServerResult::ListToolsResult(result), id).await;
        }
        ClientRequest::PingRequest(_) => {
            write_message(
                output,
                ServerResult::EmptyResult(rmcp::model::EmptyResult {}),
                id,
            )
            .await;
        }
        other => {
            write_error(
                output,
                ErrorData::new(
                    rmcp::model::ErrorCode::METHOD_NOT_FOUND,
                    format!("the recovery fixture does not serve {other:?}"),
                    None,
                ),
                id,
            )
            .await;
        }
    }
}

/// The scripted behaviour of the one generation this process serves.
#[allow(clippy::struct_excessive_bools)] // one independent switch per scripted fault
struct Generation {
    number: u64,
    dies: bool,
    corrupts: bool,
    refuses: bool,
    publishes_extra_tool: bool,
    hang_pulses: u32,
}

impl Generation {
    /// Claims this process's generation number and resolves its scripted
    /// behaviour from the environment the parent handed the binding.
    fn from_env() -> Self {
        fn generations(name: &str) -> Vec<u64> {
            std::env::var(name)
                .unwrap_or_default()
                .split(',')
                .filter_map(|entry| entry.trim().parse::<u64>().ok())
                .collect()
        }
        let number = claim_generation();
        Self {
            number,
            dies: generations(RECOVERY_DIE_GENERATIONS_ENV).contains(&number),
            corrupts: generations(RECOVERY_CORRUPT_GENERATIONS_ENV).contains(&number),
            refuses: generations(RECOVERY_REFUSE_GENERATIONS_ENV).contains(&number),
            publishes_extra_tool: generations(RECOVERY_EXTRA_TOOL_GENERATIONS_ENV)
                .contains(&number),
            hang_pulses: std::env::var(RECOVERY_HANG_PULSES_ENV)
                .ok()
                .and_then(|value| value.trim().parse().ok())
                .unwrap_or(0),
        }
    }
}

/// Answers one `tools/call` according to this generation's script.
async fn serve_call(
    output: &mut tokio::io::Stdout,
    call: CallToolRequest,
    id: RequestId,
    generation: &Generation,
    journal: Option<&PathBuf>,
    release_dir: Option<&PathBuf>,
) {
    let tool = call.params.name.to_string();
    // The dispatch gate: journaled before any per-tool behaviour, so the
    // parent proves "this request reached the server" by reading a fact the
    // server process wrote. A progress notification would be the wrong gate
    // here — an MCP client can only subscribe to a request's progress token
    // after the request is enqueued, so a notification emitted the instant
    // the server receives the call may legitimately race that subscription.
    // Progress in this fixture is therefore emitted only where the parent
    // has explicitly released it, long after any subscription exists.
    record(journal, &format!("{JOURNAL_CALL_PREFIX}{}", tool.as_str()));
    let progress_token = request_progress_token(&call);
    if generation.corrupts {
        // Well-formed JSON that is not a valid MCP message: a confirmed
        // structural protocol violation (serde `Data` class), which the
        // client's framing seam must treat as a rustX fact rather than
        // peer-only traffic.
        record(journal, JOURNAL_CORRUPTED);
        let _ = output
            .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":123}\n")
            .await;
        let _ = output.flush().await;
        return;
    }
    if generation.dies {
        // The request provably crossed the frontier (it is journaled and its
        // progress notification is on the wire) and no response will ever
        // exist: the transport dies with the call in flight.
        record(journal, JOURNAL_DIED);
        std::process::exit(0);
    }
    if tool == TOOL_ANNOUNCE
        && let Some(token) = progress_token.clone()
    {
        let mut params = ProgressNotificationParam::new(token, 1.0);
        params.total = Some(4.0);
        params.message = Some(format!("{TOOL_ANNOUNCE} working"));
        write_progress(output, params).await;
    }
    if tool == TOOL_HANG {
        // Additional remote liveness evidence, each emitted exactly when the
        // parent releases its gate, then the request is never answered. The
        // connection stays open and healthy, so only the generic hard
        // deadline can bound this call.
        for pulse in 1..=generation.hang_pulses {
            await_release(release_dir, &format!("{TOOL_HANG}.{pulse}")).await;
            if let Some(token) = progress_token.clone() {
                let mut params = ProgressNotificationParam::new(token, f64::from(pulse) + 1.0);
                params.message = Some(format!("{TOOL_HANG} pulse {pulse}"));
                write_progress(output, params).await;
            }
        }
        return;
    }
    write_message(
        output,
        ServerResult::CallToolResult(CallToolResult::success(vec![ContentBlock::text(format!(
            "{tool} ok from generation {}",
            generation.number
        ))])),
        id,
    )
    .await;
}
