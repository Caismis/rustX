//! The in-process Streamable HTTP MCP fixture for Issue #205's cancellation
//! regressions.
//!
//! The stdio recovery fixture proves the liveness and reconnection contract
//! over a real child process. It cannot prove the *Streamable HTTP*
//! cancellation contract, because the fact under test is transport-specific:
//! over HTTP a dispatched `tools/call` owns a live local HTTP request whose
//! response headers the server may withhold indefinitely, and settling that
//! call means terminating rustX's own half of it.
//!
//! # The three observation seams
//!
//! This fixture is in-process, so ordering is proven by channels rather than
//! by files or timing:
//!
//! - **`accepted`** counts `tools/call` invocations the server handler
//!   actually entered. Awaiting it proves the request crossed the network
//!   and reached the tool — strictly stronger than the effect frontier rustX
//!   classifies against — so every post-frontier claim in a test is gated on
//!   it.
//! - **`terminated`** counts invocations whose handler was cancelled because
//!   the **client disconnected its HTTP request**. rmcp's Streamable HTTP
//!   server arms a disconnect guard on the request context until the handler
//!   emits its first message, so for a tool that withholds its response this
//!   count rises exactly when rustX drops its in-flight HTTP request. It is
//!   therefore an independent, server-side proof of rustX's local request
//!   termination — the fact `Unconfirmed` settlement claims.
//! - **`release`** lets the parent decide when — or whether — the withheld
//!   correlated response is produced, so "a response beat the cancellation"
//!   and "no response can be established" are two explicit choices, never a
//!   race.
//!
//! [`TOOL_WITHHOLD`] emits nothing before it is released, so the server's
//! HTTP response for its POST has **no first message and therefore no
//! response headers** while it runs. That is the exact shape that wedges a
//! Streamable HTTP client whose outbound sends cannot progress past an
//! outstanding POST.
//!
//! **This is a test fixture, not an MCP server implementation, and must not
//! grow into one.**

use std::sync::Arc;

use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, DiscoverResult,
    ListToolsResult, PaginatedRequestParams, ProtocolVersion, ServerCapabilities, ServerInfo, Tool,
};
use rmcp::service::RequestContext;
use rmcp::{RoleServer, ServerHandler};
use tokio::sync::watch;

/// The tool that answers immediately.
pub const TOOL_ECHO: &str = "http-echo";
/// The tool that enters its handler, publishes that fact, and then produces
/// **no message at all** until the parent releases it.
///
/// Because it emits nothing, the server never sends response headers for its
/// POST: the client is left with an accepted, in-flight HTTP request and no
/// response event, which is the transport state the Issue #205 cancellation
/// contract is about.
pub const TOOL_WITHHOLD: &str = "http-withhold";
/// The tool that emits one dispatch progress notification, one further
/// gated progress notification when the parent releases it, and **never**
/// answers.
///
/// It is the concurrency vehicle for the progress-liveness contract: many of
/// these run at once, each owning its own progress token, and each one's
/// notification is genuine remote liveness evidence the generic idle
/// watchdog must see.
pub const TOOL_PULSE: &str = "http-pulse";

/// The identity of the next fixture instance.
///
/// Every fixture mints tool names that belong to exactly one instance, which
/// is what makes a tool-name-scoped test probe genuinely test-scoped: tests
/// in one binary run concurrently, and two of them asking for "the echo
/// tool" must not be the same key.
static NEXT_FIXTURE_SCOPE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// The parent's control and observation handle on one HTTP fixture.
#[derive(Clone)]
pub struct HttpFixtureControl {
    scope: u64,
    accepted: watch::Sender<u32>,
    terminated: watch::Sender<u32>,
    pulsed: watch::Sender<u32>,
    release: watch::Sender<bool>,
}

impl Default for HttpFixtureControl {
    fn default() -> Self {
        Self::new()
    }
}

impl HttpFixtureControl {
    /// A fixture control with nothing accepted, nothing terminated, and the
    /// withheld response not released.
    #[must_use]
    pub fn new() -> Self {
        Self {
            scope: NEXT_FIXTURE_SCOPE.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            accepted: watch::channel(0).0,
            terminated: watch::channel(0).0,
            pulsed: watch::channel(0).0,
            release: watch::channel(false).0,
        }
    }

    /// This fixture instance's own name for one catalog tool.
    fn scoped(&self, base: &str) -> String {
        format!("{base}-{}", self.scope)
    }

    /// This fixture's [`TOOL_ECHO`].
    #[must_use]
    pub fn echo(&self) -> String {
        self.scoped(TOOL_ECHO)
    }

    /// This fixture's [`TOOL_PULSE`].
    #[must_use]
    pub fn pulse(&self) -> String {
        self.scoped(TOOL_PULSE)
    }

    /// This fixture's [`TOOL_WITHHOLD`].
    #[must_use]
    pub fn withhold(&self) -> String {
        self.scoped(TOOL_WITHHOLD)
    }

    /// Resolves once at least `count` `tools/call` invocations have entered
    /// the server handler.
    ///
    /// # Panics
    ///
    /// Panics if the fixture's observation channel closed.
    pub async fn wait_accepted(&self, count: u32) {
        self.accepted
            .subscribe()
            .wait_for(|accepted| *accepted >= count)
            .await
            .expect("the fixture accept channel stays open");
    }

    /// Resolves once at least `count` invocations have been cancelled by a
    /// client HTTP disconnect.
    ///
    /// This is the server-side proof that rustX terminated its own in-flight
    /// HTTP request for that call.
    ///
    /// # Panics
    ///
    /// Panics if the fixture's observation channel closed.
    pub async fn wait_terminated(&self, count: u32) {
        self.terminated
            .subscribe()
            .wait_for(|terminated| *terminated >= count)
            .await
            .expect("the fixture termination channel stays open");
    }

    /// Releases every withheld invocation, so each produces its correlated
    /// remote response.
    pub fn release(&self) {
        self.release.send_replace(true);
    }

    /// How many invocations entered the handler.
    #[must_use]
    pub fn accepted_calls(&self) -> u32 {
        *self.accepted.borrow()
    }

    /// How many invocations the client's HTTP disconnect cancelled.
    #[must_use]
    pub fn terminated_calls(&self) -> u32 {
        *self.terminated.borrow()
    }

    /// Resolves once at least `count` [`TOOL_PULSE`] invocations have sent
    /// their released progress notification.
    ///
    /// # Panics
    ///
    /// Panics if the fixture's observation channel closed.
    pub async fn wait_pulsed(&self, count: u32) {
        self.pulsed
            .subscribe()
            .wait_for(|pulsed| *pulsed >= count)
            .await
            .expect("the fixture pulse channel stays open");
    }
}

/// The fixture MCP server itself.
#[derive(Clone)]
pub struct HttpFixtureServer {
    control: HttpFixtureControl,
}

impl HttpFixtureServer {
    /// A server bound to one parent-owned control handle.
    #[must_use]
    pub fn new(control: HttpFixtureControl) -> Self {
        Self { control }
    }

    fn catalog(&self) -> Vec<Tool> {
        vec![
            super::fixture_tool_named(&self.control.echo()),
            super::fixture_tool_named(&self.control.pulse()),
            super::fixture_tool_named(&self.control.withhold()),
        ]
    }

    /// Sends one progress notification for the call's own progress token.
    async fn notify(
        context: &RequestContext<RoleServer>,
        progress: f64,
    ) -> Result<(), rmcp::ErrorData> {
        let Some(token) = context.meta.get_progress_token() else {
            return Err(rmcp::ErrorData::internal_error(
                "the fixture requires a progress token",
                None,
            ));
        };
        context
            .peer
            .notify_progress(rmcp::model::ProgressNotificationParam::new(token, progress))
            .await
            .map_err(|error| {
                rmcp::ErrorData::internal_error(format!("cannot notify progress: {error}"), None)
            })
    }
}

impl ServerHandler for HttpFixtureServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
    }

    fn supported_protocol_versions(&self) -> std::borrow::Cow<'static, [ProtocolVersion]> {
        std::borrow::Cow::Owned(vec![ProtocolVersion::V_2026_07_28])
    }

    fn discover(
        &self,
        _context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<DiscoverResult, rmcp::ErrorData>> + Send {
        std::future::ready(Ok(DiscoverResult::from_server_info(
            vec![ProtocolVersion::V_2026_07_28],
            self.get_info(),
        )))
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        self.catalog().into_iter().find(|tool| tool.name == *name)
    }

    fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<ListToolsResult, rmcp::ErrorData>> + Send {
        std::future::ready(Ok(ListToolsResult {
            tools: self.catalog(),
            ..Default::default()
        }))
    }

    fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<CallToolResponse, rmcp::ErrorData>> + Send {
        let control = self.control.clone();
        let echo = control.echo();
        let pulse = control.pulse();
        let withhold = control.withhold();
        async move {
            match request.name.as_ref() {
                name if name == echo => {
                    // Counted like every other tool of this fixture, so
                    // `accepted_calls() == 0` is evidence that a request
                    // never arrived rather than evidence that this tool
                    // never counted.
                    control.accepted.send_modify(|accepted| *accepted += 1);
                    Ok(CallToolResult::success(vec![ContentBlock::text("http echo")]).into())
                }
                name if name == pulse => {
                    Self::notify(&context, 1.0).await?;
                    control.accepted.send_modify(|accepted| *accepted += 1);
                    let mut release = control.release.subscribe();
                    release.wait_for(|released| *released).await.map_err(|_| {
                        rmcp::ErrorData::internal_error("the fixture control closed", None)
                    })?;
                    Self::notify(&context, 2.0).await?;
                    control.pulsed.send_modify(|pulsed| *pulsed += 1);
                    // Never answers: the call stays in flight until the
                    // generic lifecycle bounds it.
                    context.ct.cancelled().await;
                    control
                        .terminated
                        .send_modify(|terminated| *terminated += 1);
                    Err(rmcp::ErrorData::internal_error(
                        "the client terminated its HTTP request",
                        None,
                    ))
                }
                name if name == withhold => {
                    control.accepted.send_modify(|accepted| *accepted += 1);
                    let mut release = control.release.subscribe();
                    tokio::select! {
                        () = context.ct.cancelled() => {
                            // The client disconnected its HTTP request while
                            // this handler had produced no message. That is
                            // rustX terminating its own local request
                            // ownership, observed from the far side.
                            control.terminated.send_modify(|terminated| *terminated += 1);
                            Err(rmcp::ErrorData::internal_error(
                                "the client terminated its HTTP request",
                                None,
                            ))
                        }
                        released = release.wait_for(|released| *released) => {
                            match released {
                                Ok(_) => Ok(CallToolResult::success(vec![ContentBlock::text(
                                    "http released",
                                )])
                                .into()),
                                Err(_) => Err(rmcp::ErrorData::internal_error(
                                    "the fixture control channel closed",
                                    None,
                                )),
                            }
                        }
                    }
                }
                _ => Err(rmcp::ErrorData::method_not_found::<
                    rmcp::model::CallToolRequestMethod,
                >()),
            }
        }
    }
}

/// One running Streamable HTTP fixture server.
///
/// Dropping it is not the shutdown path: call [`HttpFixture::shutdown`], which
/// cancels the server and joins its task, so no fixture listener outlives the
/// test that created it.
pub struct HttpFixture {
    /// The endpoint an `McpTransportConfig::StreamableHttp` binding uses.
    pub endpoint: String,
    /// The parent's control and observation handle.
    pub control: HttpFixtureControl,
    cancellation: tokio_util::sync::CancellationToken,
    server: tokio::task::JoinHandle<()>,
}

impl HttpFixture {
    /// Binds a loopback listener and serves the fixture on `/mcp`.
    ///
    /// # Panics
    ///
    /// Panics if the loopback listener cannot be bound.
    pub async fn start(control: HttpFixtureControl) -> Self {
        use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
        use rmcp::transport::streamable_http_server::{
            StreamableHttpServerConfig, StreamableHttpService,
        };

        let cancellation = tokio_util::sync::CancellationToken::new();
        let mut config = StreamableHttpServerConfig::default();
        config.cancellation_token = cancellation.child_token();
        config.sse_keep_alive = None;
        let handler = HttpFixtureServer::new(control.clone());
        let service = StreamableHttpService::<HttpFixtureServer, LocalSessionManager>::new(
            move || Ok(handler.clone()),
            Arc::default(),
            config,
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("fixture HTTP listener");
        let address = listener.local_addr().expect("fixture HTTP address");
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, axum::Router::new().nest_service("/mcp", service)).await;
        });
        Self {
            endpoint: format!("http://{address}/mcp"),
            control,
            cancellation,
            server,
        }
    }

    /// The binding one capability owner uses to reach this fixture.
    #[must_use]
    pub fn binding(&self) -> crate::tools::mcp::McpServerBinding {
        self.binding_with_header(None)
    }

    /// The same binding with one extra static request header.
    ///
    /// A header is an execution-relevant field of `McpServerBinding`, so this
    /// is how a test constructs a *different* binding for the same endpoint.
    #[must_use]
    pub fn binding_with_header(
        &self,
        header: Option<(&str, &str)>,
    ) -> crate::tools::mcp::McpServerBinding {
        let headers = header
            .into_iter()
            .map(|(name, value)| (name.to_owned(), value.to_owned()))
            .collect();
        crate::tools::mcp::McpServerBinding {
            transport: crate::tools::mcp::McpTransportConfig::StreamableHttp {
                endpoint: self.endpoint.clone(),
                headers,
            },
            policy: crate::tools::types::ToolInvocationPolicy::default(),
        }
    }

    /// Cancels the server and joins its task.
    pub async fn shutdown(self) {
        self.cancellation.cancel();
        self.server.abort();
        let _ = self.server.await;
    }
}
