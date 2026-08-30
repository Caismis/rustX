//! Official-rmcp fixture server shared by the M7 local integration tests.
//!
//! Feature-gated behind `mcp-fixture`; never used by production code. The
//! fixture is served either in-process (Streamable HTTP) or as a self-spawned
//! stdio server (the test binary re-runs itself in fixture mode).
//!
//! The fixture exposes:
//!
//! - `echo` — deterministic success;
//! - `mutate` — flips the catalog, emits one fractional progress
//!   notification, then a `tools/list_changed` notification;
//! - `slow` — notifies call-start, awaits the server-side cancellation
//!   context (proving the client's cancellation notification reached the
//!   server), records the observation, and returns;
//! - when pagination is enabled, a multi-page `tools/list` catalog of
//!   `[alpha, beta, gamma, delta, echo]` served two tools per page.
//!
//! [`legacy`] is the one deliberate exception to the official-rmcp rule: a
//! hand-written pre-2026 wire fixture for the one peer shape an rmcp server
//! cannot represent.

pub mod legacy;

use std::borrow::Cow;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, DiscoverResult,
    JsonObject, PaginatedRequestParams, ProgressNotificationParam, ProtocolVersion,
    ServerCapabilities, ServerInfo, SubscriptionFilter, Tool,
};
use rmcp::service::{RequestContext, SubscriptionContext};
use rmcp::{RoleServer, ServerHandler, ServiceExt};

/// The environment variable selecting fixture mode when the test binary is
/// re-executed as its own stdio MCP server.
pub const FIXTURE_MODE_ENV: &str = "RUSTX_M7_MCP_FIXTURE";
/// The environment variable naming the marker file the `slow` tool writes
/// the moment its server-side cancellation context fires (self-spawned
/// stdio fixtures, where the fixture state lives in another process).
pub const CANCEL_FILE_ENV: &str = "RUSTX_M7_FIXTURE_CANCEL_FILE";
/// The environment variable selecting the paginated `tools/list` catalog
/// page size (self-spawned stdio fixtures).
pub const PAGE_SIZE_ENV: &str = "RUSTX_M7_FIXTURE_PAGE_SIZE";
/// The environment variable prefixing fixture tool names. This is primarily
/// a multi-server test seam: MCP tool names are model-facing and therefore
/// must be unique across simultaneously active fixture servers.
pub const TOOL_PREFIX_ENV: &str = "RUSTX_M7_FIXTURE_TOOL_PREFIX";
/// The environment variable narrowing the protocol revisions the fixture
/// server supports, as a comma-separated list (self-spawned stdio fixtures).
///
/// This is the deterministic protocol-negotiation seam: the value flows
/// straight into rmcp's `ServerHandler::supported_protocol_versions`, which
/// bounds `server/discover` advertisement, `initialize` negotiation, and
/// per-request version validation alike.
pub const PROTOCOL_VERSIONS_ENV: &str = "RUSTX_M7_FIXTURE_PROTOCOL_VERSIONS";
/// The environment variable making the fixture's `tools/list` fail with a
/// correlated error message of exactly the given byte length
/// (self-spawned stdio fixtures).
///
/// This is the deterministic oversized-diagnostic seam (Issue #81): an
/// external MCP peer can produce an arbitrarily large failure payload, and
/// the capability availability contract must bound it before the
/// diagnostic enters authoritative state.
pub const LIST_TOOLS_ERROR_BYTES_ENV: &str = "RUSTX_M7_FIXTURE_LIST_TOOLS_ERROR_BYTES";
/// The environment variable selecting the byte length of the successful
/// `echo` text result (Issue #103).
pub const RESULT_BYTES_ENV: &str = "RUSTX_M7_FIXTURE_RESULT_BYTES";
/// The environment variable selecting comma-separated successful `echo`
/// content-block byte lengths. Blocks are joined by the Tool Plane's
/// deterministic newline representation, so their aggregate can cross the
/// bound even when each block is individually below it.
pub const RESULT_BLOCK_BYTES_ENV: &str = "RUSTX_M7_FIXTURE_RESULT_BLOCK_BYTES";
/// The environment variable naming a marker file to which each successful
/// `echo` call appends one line. This keeps exactly-once assertions
/// deterministic for self-spawned stdio fixtures, whose server state lives
/// in another process.
pub const ECHO_CALL_COUNT_FILE_ENV: &str = "RUSTX_M7_FIXTURE_ECHO_CALL_COUNT_FILE";
/// Parses a comma-separated protocol revision list.
///
/// Every MCP revision string is accepted, including ones no SDK knows: that
/// is exactly what an unsupported-revision fixture needs.
#[must_use]
pub fn parse_protocol_versions(value: &str) -> Vec<ProtocolVersion> {
    value
        .split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(|entry| {
            serde_json::from_value::<ProtocolVersion>(serde_json::Value::String(entry.to_owned()))
                .expect("a protocol revision string always deserializes")
        })
        .collect()
}

impl FixtureServer {
    /// Builds a fixture from the self-spawn environment: cancellation
    /// marker file, pagination page size, and protocol behavior, when set.
    #[must_use]
    pub fn from_env() -> Self {
        Self {
            list_changed_supported: true,
            cancel_observed_file: std::env::var_os(CANCEL_FILE_ENV).map(PathBuf::from),
            page_size: std::env::var(PAGE_SIZE_ENV)
                .ok()
                .and_then(|value| value.parse::<usize>().ok()),
            tool_prefix: std::env::var(TOOL_PREFIX_ENV).ok(),
            supported_versions: std::env::var(PROTOCOL_VERSIONS_ENV)
                .ok()
                .map(|value| parse_protocol_versions(&value)),
            list_tools_error_bytes: std::env::var(LIST_TOOLS_ERROR_BYTES_ENV)
                .ok()
                .and_then(|value| value.parse::<usize>().ok()),
            result_bytes: std::env::var(RESULT_BYTES_ENV)
                .ok()
                .and_then(|value| value.parse::<usize>().ok()),
            result_block_bytes: std::env::var(RESULT_BLOCK_BYTES_ENV).ok().map(|value| {
                value
                    .split(',')
                    .filter_map(|entry| entry.parse::<usize>().ok())
                    .collect()
            }),
            echo_call_count_file: std::env::var_os(ECHO_CALL_COUNT_FILE_ENV).map(PathBuf::from),
            ..Self::default()
        }
    }

    /// The revisions this fixture serves, newest first.
    fn versions(&self) -> Vec<ProtocolVersion> {
        self.supported_versions.clone().unwrap_or_else(|| {
            let mut versions = ProtocolVersion::KNOWN_VERSIONS.to_vec();
            versions.sort_by(|left, right| right.as_str().cmp(left.as_str()));
            versions
        })
    }

    /// The fixture tool catalog for the current state. In pagination mode
    /// the catalog is the finite five-tool set served two tools per page.
    fn catalog(&self) -> Vec<Tool> {
        if let Some(page_size) = self.page_size {
            let _ = page_size;
            return vec![
                self.fixture_tool_named("alpha"),
                self.fixture_tool_named("beta"),
                self.fixture_tool_named("delta"),
                self.fixture_tool_named("echo"),
                self.fixture_tool_named("gamma"),
            ];
        }
        if self.changed.load(Ordering::Acquire) {
            vec![
                self.fixture_tool_named("echo"),
                self.fixture_tool_named("new_tool"),
            ]
        } else {
            vec![
                self.fixture_tool_named("echo"),
                self.fixture_tool_named("mutate"),
                self.fixture_tool_named("slow"),
            ]
        }
    }

    fn fixture_tool_named(&self, name: &str) -> Tool {
        let name = self.tool_name(name);
        fixture_tool_named(&name)
    }

    fn tool_name(&self, name: &str) -> String {
        self.tool_prefix
            .as_deref()
            .map_or_else(|| name.to_owned(), |prefix| format!("{prefix}{name}"))
    }
}

/// The shared observable state of one fixture instance.
#[derive(Clone, Default)]
pub struct FixtureServer {
    /// The catalog flip observed by `tools/list`.
    pub changed: Arc<AtomicBool>,
    /// The current subscription sink, installed by `listen`.
    pub sink: Arc<tokio::sync::Mutex<Option<rmcp::service::SubscriptionSink>>>,
    /// Fired when the `slow` tool starts executing server-side.
    pub slow_started: Arc<tokio::sync::Notify>,
    /// Fired when the `slow` tool's server-side cancellation context
    /// becomes observable.
    pub cancel_observed: Arc<tokio::sync::Notify>,
    /// When set, the `slow` tool additionally writes a marker file the
    /// moment its cancellation context fires (for self-spawned stdio
    /// fixtures, where the fixture state lives in another process).
    pub cancel_observed_file: Option<PathBuf>,
    /// Whether the server advertises and accepts `tools/list_changed`.
    pub list_changed_supported: bool,
    /// When set, `tools/list` paginates its catalog with this page size.
    pub page_size: Option<usize>,
    /// When set, the exact protocol revisions this server supports; `None`
    /// means every revision the SDK knows.
    pub supported_versions: Option<Vec<ProtocolVersion>>,
    /// Optional prefix applied to model-facing fixture tool names.
    pub tool_prefix: Option<String>,
    /// The number of `subscriptions/listen` streams this server has accepted.
    ///
    /// A client that installed more than one invalidation mechanism per
    /// connection shows up here as a count above one.
    pub listen_calls: Arc<std::sync::atomic::AtomicUsize>,
    /// When set, `tools/list` fails with a correlated error message of
    /// exactly this many bytes (the oversized-diagnostic seam).
    pub list_tools_error_bytes: Option<usize>,
    /// When set, the successful `echo` tool returns one text block of exactly
    /// this many bytes.
    pub result_bytes: Option<usize>,
    /// When set, the successful `echo` tool returns one text block for each
    /// listed byte length.
    pub result_block_bytes: Option<Vec<usize>>,
    /// When set, each successful `echo` invocation appends one line to this
    /// file for deterministic cross-process exactly-once assertions.
    pub echo_call_count_file: Option<PathBuf>,
}

impl FixtureServer {
    /// A fixture with `tools/list_changed` support.
    #[must_use]
    pub fn with_list_changed() -> Self {
        Self {
            list_changed_supported: true,
            ..Self::default()
        }
    }
}

fn record_echo_call(path: Option<&std::path::Path>) -> Result<(), rmcp::ErrorData> {
    let Some(path) = path else {
        return Ok(());
    };
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|error| {
            rmcp::ErrorData::internal_error(
                format!("cannot record fixture echo call: {error}"),
                None,
            )
        })?;
    writeln!(file, "echo").map_err(|error| {
        rmcp::ErrorData::internal_error(format!("cannot record fixture echo call: {error}"), None)
    })
}

impl ServerHandler for FixtureServer {
    fn get_info(&self) -> ServerInfo {
        let mut capabilities = ServerCapabilities::builder().enable_tools();
        if self.list_changed_supported {
            capabilities = capabilities.enable_tool_list_changed();
        }
        let mut info = ServerInfo::new(capabilities.build());
        // The legacy `initialize` fallback echoes this revision whenever the
        // client asks for one the fixture does not serve, so it must be a
        // revision the fixture really serves.
        if let Some(newest) = self.versions().first() {
            info.protocol_version = newest.clone();
        }
        info
    }

    fn supported_protocol_versions(&self) -> Cow<'static, [ProtocolVersion]> {
        Cow::Owned(self.versions())
    }

    fn discover(
        &self,
        _context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<DiscoverResult, rmcp::ErrorData>> + Send {
        // Overridden only so the advertised set matches `versions()` even
        // when the fixture narrows it.
        std::future::ready(Ok(DiscoverResult::from_server_info(
            self.versions(),
            self.get_info(),
        )))
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        Some(fixture_tool_named(name))
    }

    fn list_tools(
        &self,
        request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<rmcp::model::ListToolsResult, rmcp::ErrorData>> + Send
    {
        if let Some(bytes) = self.list_tools_error_bytes {
            let message = format!("catalog unavailable: {}", "x".repeat(bytes));
            return std::future::ready(Err(rmcp::ErrorData::internal_error(message, None)));
        }
        let tools = self.catalog();
        let Some(page_size) = self.page_size else {
            let result = rmcp::model::ListToolsResult {
                tools,
                ..Default::default()
            };
            return std::future::ready(Ok(result));
        };
        // Cursor-based pagination: the cursor is the index of the first tool
        // of the next page; `None` starts at page zero.
        let cursor = request.and_then(|request| request.cursor);
        let start = cursor
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(0);
        let next_start = start + page_size;
        let page = tools
            .into_iter()
            .skip(start)
            .take(page_size)
            .collect::<Vec<_>>();
        let next_cursor = if next_start < self.catalog().len() {
            Some(next_start.to_string())
        } else {
            None
        };
        std::future::ready(Ok(rmcp::model::ListToolsResult {
            tools: page,
            next_cursor,
            ..Default::default()
        }))
    }

    fn accepted_subscription_filter(
        &self,
        _requested: &SubscriptionFilter,
    ) -> Option<SubscriptionFilter> {
        self.list_changed_supported
            .then(|| SubscriptionFilter::builder().tools_list_changed().build())
    }

    async fn listen(&self, context: SubscriptionContext) -> Result<(), rmcp::ErrorData> {
        self.listen_calls.fetch_add(1, Ordering::Release);
        *self.sink.lock().await = Some(context.sink().clone());
        context.cancelled().await;
        Ok(())
    }

    fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<CallToolResponse, rmcp::ErrorData>> + Send {
        let changed = self.changed.clone();
        let sink = self.sink.clone();
        let slow_started = self.slow_started.clone();
        let cancel_observed = self.cancel_observed.clone();
        let cancel_observed_file = self.cancel_observed_file.clone();
        let result_bytes = self.result_bytes;
        let result_block_bytes = self.result_block_bytes.clone();
        let echo_call_count_file = self.echo_call_count_file.clone();
        let echo_name = self.tool_name("echo");
        let mutate_name = self.tool_name("mutate");
        let slow_name = self.tool_name("slow");
        async move {
            if request.name == echo_name {
                record_echo_call(echo_call_count_file.as_deref())?;
                let blocks = result_block_bytes.map_or_else(
                    || {
                        vec![ContentBlock::text(result_bytes.map_or_else(
                            || "fixture echo".to_owned(),
                            |bytes| "x".repeat(bytes),
                        ))]
                    },
                    |blocks| {
                        blocks
                            .into_iter()
                            .map(|bytes| ContentBlock::text("x".repeat(bytes)))
                            .collect()
                    },
                );
                Ok(CallToolResult::success(blocks).into())
            } else if request.name == mutate_name {
                changed.store(true, Ordering::Release);
                if let Some(token) = context.meta.get_progress_token() {
                    context
                        .peer
                        .notify_progress(
                            ProgressNotificationParam::new(token, 0.5)
                                .with_total(3.5)
                                .with_message("fractional"),
                        )
                        .await
                        .map_err(|error| {
                            rmcp::ErrorData::internal_error(
                                format!("cannot notify progress: {error}"),
                                None,
                            )
                        })?;
                }
                // Inline-lifecycle clients open a `subscriptions/listen`
                // stream; legacy clients get the plain
                // `notifications/tools/list_changed` their revision defines.
                if let Some(sink) = sink.lock().await.clone() {
                    sink.notify_tool_list_changed().await.map_err(|error| {
                        rmcp::ErrorData::internal_error(
                            format!("cannot notify tool list change: {error}"),
                            None,
                        )
                    })?;
                } else {
                    context
                        .peer
                        .notify_tool_list_changed()
                        .await
                        .map_err(|error| {
                            rmcp::ErrorData::internal_error(
                                format!("cannot notify tool list change: {error}"),
                                None,
                            )
                        })?;
                }
                Ok(CallToolResult::success(vec![ContentBlock::text("fixture changed")]).into())
            } else if request.name == slow_name {
                if let Some(token) = context.meta.get_progress_token() {
                    context
                        .peer
                        .notify_progress(ProgressNotificationParam::new(token, 0.25))
                        .await
                        .map_err(|error| {
                            rmcp::ErrorData::internal_error(
                                format!("cannot notify slow-call progress: {error}"),
                                None,
                            )
                        })?;
                }
                slow_started.notify_one();
                context.ct.cancelled().await;
                cancel_observed.notify_one();
                if let Some(path) = &cancel_observed_file {
                    let _ = std::fs::write(path, "cancel_observed");
                }
                Ok(CallToolResult::success(vec![ContentBlock::text("fixture cancelled")]).into())
            } else {
                Err(rmcp::ErrorData::method_not_found::<
                    rmcp::model::CallToolRequestMethod,
                >())
            }
        }
    }
}

/// Builds one canonical fixture tool definition.
#[must_use]
pub fn fixture_tool_named(name: &str) -> Tool {
    let mut tool = Tool::default();
    tool.name = name.to_owned().into();
    tool.description = Some(format!("fixture {name}").into());
    let mut schema = JsonObject::new();
    schema.insert("type".to_owned(), serde_json::json!("object"));
    schema.insert("properties".to_owned(), serde_json::json!({}));
    schema.insert("additionalProperties".to_owned(), serde_json::json!(false));
    tool.input_schema = Arc::new(schema);
    tool
}

/// Serves one fixture over stdio until the client closes the transport.
pub async fn serve_stdio(fixture: FixtureServer) {
    let server = fixture
        .serve(rmcp::transport::stdio())
        .await
        .expect("fixture server");
    server.waiting().await.expect("fixture server wait");
}

/// Runs the current test binary as a stdio fixture server, when
/// [`FIXTURE_MODE_ENV`] selects fixture mode.
///
/// Every M7 local stdio test starts with this branch: with the fixture env
/// variable set, the re-executed binary serves the fixture and returns, so
/// the parent test process can drive the exact same fixture through the real
/// rustX `McpServerRuntime` stdio transport.
pub async fn serve_if_fixture_mode(fixture: FixtureServer) -> bool {
    if std::env::var_os(FIXTURE_MODE_ENV).is_some() {
        serve_stdio(fixture).await;
        true
    } else {
        false
    }
}

/// The argument vector that re-runs the current test binary as exactly this
/// test in fixture mode.
#[must_use]
pub fn fixture_spawn_args(test_name: &str) -> Vec<String> {
    vec![
        "--exact".to_owned(),
        test_name.to_owned(),
        "--quiet".to_owned(),
        "--nocapture".to_owned(),
        "--test-threads".to_owned(),
        "1".to_owned(),
    ]
}
