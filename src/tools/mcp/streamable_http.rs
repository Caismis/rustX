//! rustX-owned local ownership of Streamable HTTP MCP requests (Issue #205).
//!
//! # Why this layer exists
//!
//! `notifications/cancelled` is the strongest cancellation the MCP protocol
//! defines, and it is **remote control**: the peer may honour it, ignore it,
//! or never see it. It says nothing about the local half of a dispatched
//! request. Over stdio there is no local half worth naming — an outbound
//! write owns no resource that outlives it — but over Streamable HTTP every
//! `tools/call` is an HTTP request whose POST future and whose response body
//! stream are live local objects for as long as the server keeps them open.
//!
//! Issue #204 requires that [`crate::tools::executor::ToolSettlement::Unconfirmed`]
//! mean *all rustX-owned local execution and cleanup of the invocation is
//! settled; only the remote effect is uncertain*. Satisfying that over HTTP
//! requires rustX to own, terminate, and **prove the release of** that local
//! half — not to hope a protocol notification did it, and not to drop a
//! future and call the drop a proof.
//!
//! ```text
//! remote external outcome        rustX/rmcp local request ownership
//!   owned by: the server           owned by: this module
//!   proven by: a correlated        proven by: the release latch of the
//!              response                       request's ownership guard
//! ```
//!
//! # The ownership model
//!
//! [`McpHttpRequestOwnership`] is the per-connection-generation registry of
//! the HTTP requests rustX currently owns locally, keyed by JSON-RPC request
//! id. [`McpHttpClient`] — the [`StreamableHttpClient`] rmcp actually posts
//! through — registers every request-carrying POST there and holds its
//! [`RequestOwnershipGuard`] for exactly as long as any local HTTP activity
//! of that request exists: the POST future while it awaits response headers,
//! and then the SSE response body stream, which the guard travels into.
//!
//! Terminating one request cancels that guard's token. The wrapper reacts by
//! **dropping the inner POST future or the inner response body first, and
//! releasing the latch only afterwards**, so awaiting the latch is a real
//! ownership proof rather than a restatement of "we stopped waiting".
//!
//! A request that has been terminated before its POST ever started is
//! recorded in a small ledger, so the POST is pre-terminated when it does
//! start and can never leave rustX after settlement. That closes the
//! dispatch/cancellation race without any timer.
//!
//! # Boundedness
//!
//! Everything here is bounded by construction: one entry per HTTP request
//! rustX currently owns (which is one per in-flight MCP request of this
//! generation), plus a fixed-size FIFO ledger of pre-terminated ids. No task
//! is spawned, nothing is retried, and nothing survives the connection
//! generation: closing the runtime terminates every entry, and joining
//! rmcp's transport worker consumes the futures that hold the guards.

use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use futures_util::stream::BoxStream;
use futures_util::{Stream, StreamExt as _};
use http::{HeaderName, HeaderValue};
use rmcp::model::{ClientJsonRpcMessage, RequestId};
use rmcp::transport::streamable_http_client::{
    SseError, StreamableHttpClient, StreamableHttpError, StreamableHttpPostResponse,
};
use sse_stream::Sse;
use tokio_util::sync::{CancellationToken, WaitForCancellationFutureOwned};

/// The Server-Sent Events stream shape of rmcp's Streamable HTTP client.
type SseStream = BoxStream<'static, Result<Sse, SseError>>;
/// The header map every [`StreamableHttpClient`] method takes.
type CustomHeaders = HashMap<HeaderName, HeaderValue>;

/// How many request ids terminated before their POST started are remembered.
///
/// The ledger only has to outlive the window between rmcp accepting a
/// request on its outbound queue and its POST actually starting, so it is
/// bounded by the requests in flight on one transport rather than by
/// anything a peer controls. Eviction is FIFO: the entry a settlement just
/// recorded is the newest and is the last one an overflow can reach.
const MAX_PRE_TERMINATED_REQUESTS: usize = 64;

/// A one-shot latch that resolves once its request's local HTTP ownership
/// has been released.
#[derive(Default)]
pub(crate) struct ReleaseLatch {
    released: AtomicBool,
    notify: tokio::sync::Notify,
}

impl ReleaseLatch {
    fn release(&self) {
        self.released.store(true, Ordering::Release);
        self.notify.notify_waiters();
    }

    async fn released(&self) {
        // Register before the check: a release racing this call must not be
        // able to fall between the two.
        let notified = self.notify.notified();
        if self.released.load(Ordering::Acquire) {
            return;
        }
        notified.await;
    }
}

struct LiveRequest {
    terminate: CancellationToken,
    release: Arc<ReleaseLatch>,
}

#[derive(Default)]
struct OwnershipState {
    /// The HTTP requests rustX currently owns locally.
    live: HashMap<RequestId, LiveRequest>,
    /// Requests terminated before their POST started, so it never starts.
    pre_terminated: VecDeque<RequestId>,
}

/// The rustX-owned local HTTP request ownership of one Streamable HTTP
/// connection generation.
#[derive(Default)]
pub(crate) struct McpHttpRequestOwnership {
    state: Mutex<OwnershipState>,
}

/// What terminating one request's local HTTP ownership found, and the proof
/// that it is settled.
///
/// `settled()` is the executor's local settlement bound. It depends on
/// rustX-owned state only: no remote response, no protocol acknowledgement,
/// and no timer participates in it.
pub(crate) enum LocalRequestTermination {
    /// This transport owns no per-request local HTTP state, or the request
    /// owns none right now. Either way nothing rustX owns for this
    /// invocation is still running.
    Settled,
    /// An in-flight local HTTP request was terminated; the latch resolves
    /// once its POST future or response body has actually been dropped.
    Terminated(Arc<ReleaseLatch>),
}

impl LocalRequestTermination {
    /// Whether rustX actually had a live local HTTP request to terminate.
    ///
    /// A call that terminated its own transport-level request explains the
    /// transport-send failure its dispatch then observes, so that failure is
    /// not independent evidence that the connection generation died.
    pub(crate) const fn terminated_local_request(&self) -> bool {
        matches!(self, Self::Terminated(_))
    }

    /// Awaits the proof that no rustX-owned local HTTP activity of this
    /// request remains.
    pub(crate) async fn settled(&self) {
        match self {
            Self::Settled => (),
            Self::Terminated(latch) => latch.released().await,
        }
    }
}

impl McpHttpRequestOwnership {
    /// Registers one outbound request and returns the guard that owns its
    /// local HTTP activity.
    fn register(self: &Arc<Self>, id: RequestId) -> RequestOwnershipGuard {
        let terminate = CancellationToken::new();
        let release = Arc::new(ReleaseLatch::default());
        {
            let mut state = self.state.lock().expect("MCP HTTP ownership lock poisoned");
            if let Some(index) = state
                .pre_terminated
                .iter()
                .position(|terminated| terminated == &id)
            {
                // Settlement already terminated this request before its POST
                // started: it must not reach the network now.
                state.pre_terminated.remove(index);
                terminate.cancel();
            }
            state.live.insert(
                id.clone(),
                LiveRequest {
                    terminate: terminate.clone(),
                    release: Arc::clone(&release),
                },
            );
        }
        RequestOwnershipGuard {
            ownership: Arc::clone(self),
            id,
            terminate,
            release,
        }
    }

    /// Terminates the local HTTP ownership of one request.
    ///
    /// This is synchronous and unconditional: it cancels the request's token
    /// (ending its POST future or its response body stream) and, when no
    /// POST has started yet, records the id so the one that starts later is
    /// pre-terminated instead of reaching the network.
    pub(crate) fn terminate(&self, id: &RequestId) -> LocalRequestTermination {
        let mut state = self.state.lock().expect("MCP HTTP ownership lock poisoned");
        if let Some(live) = state.live.get(id) {
            live.terminate.cancel();
            return LocalRequestTermination::Terminated(Arc::clone(&live.release));
        }
        if !state.pre_terminated.iter().any(|known| known == id) {
            if state.pre_terminated.len() == MAX_PRE_TERMINATED_REQUESTS {
                state.pre_terminated.pop_front();
            }
            state.pre_terminated.push_back(id.clone());
        }
        LocalRequestTermination::Settled
    }

    /// Terminates every request this generation still owns locally.
    ///
    /// Connection close calls this before it awaits rmcp's transport
    /// shutdown, so drain owns the per-request control primitives introduced
    /// here rather than inheriting them.
    pub(crate) fn terminate_all(&self) {
        let state = self.state.lock().expect("MCP HTTP ownership lock poisoned");
        for live in state.live.values() {
            live.terminate.cancel();
        }
    }

    /// Releases one request's ownership, but only if the entry is still the
    /// one this guard created.
    fn release(&self, id: &RequestId, release: &Arc<ReleaseLatch>) {
        {
            let mut state = self.state.lock().expect("MCP HTTP ownership lock poisoned");
            if state
                .live
                .get(id)
                .is_some_and(|live| Arc::ptr_eq(&live.release, release))
            {
                state.live.remove(id);
            }
        }
        release.release();
    }
}

/// The ownership of one request's local HTTP activity.
///
/// Dropping it is what releases the latch, so it is held by whichever local
/// object is still alive: the POST future, and then the response body
/// stream it produced.
struct RequestOwnershipGuard {
    ownership: Arc<McpHttpRequestOwnership>,
    id: RequestId,
    terminate: CancellationToken,
    release: Arc<ReleaseLatch>,
}

impl Drop for RequestOwnershipGuard {
    fn drop(&mut self) {
        self.ownership.release(&self.id, &self.release);
    }
}

/// One request-scoped Streamable HTTP response body owned by rustX.
///
/// The ordering inside `poll_next` is the whole point: on termination the
/// inner byte stream is dropped **first**, and the ownership guard is
/// released only afterwards. The release latch is therefore a proof that
/// the HTTP response body of this request is gone, not merely that this
/// stream stopped being polled.
/// Field order is load-bearing: Rust drops fields in declaration order, so
/// dropping the whole stream also drops the response body before it releases
/// the guard.
struct OwnedResponseStream<S> {
    inner: Option<S>,
    terminated: Pin<Box<WaitForCancellationFutureOwned>>,
    guard: Option<RequestOwnershipGuard>,
}

impl<S: Stream + Unpin> Stream for OwnedResponseStream<S> {
    type Item = S::Item;

    fn poll_next(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        let Some(inner) = this.inner.as_mut() else {
            return Poll::Ready(None);
        };
        if this.terminated.as_mut().poll(context).is_ready() {
            // Drop order is the ownership proof.
            this.inner = None;
            this.guard = None;
            return Poll::Ready(None);
        }
        match Pin::new(inner).poll_next(context) {
            Poll::Ready(None) => {
                this.inner = None;
                this.guard = None;
                Poll::Ready(None)
            }
            other => other,
        }
    }
}

/// The HTTP client rmcp's Streamable HTTP transport posts through.
///
/// It is an ownership seam, not a policy layer: every request-carrying POST
/// is registered with [`McpHttpRequestOwnership`] and nothing else about the
/// exchange changes. Requests without a JSON-RPC id (notifications and
/// responses) own no correlated local state a settlement could ever need to
/// terminate, so they pass straight through.
#[derive(Clone)]
pub(crate) struct McpHttpClient {
    inner: reqwest::Client,
    ownership: Arc<McpHttpRequestOwnership>,
}

impl McpHttpClient {
    /// Builds the client of one Streamable HTTP connection generation,
    /// together with the ownership registry its settlements terminate
    /// through.
    ///
    /// The HTTP client configuration matches the one rmcp builds for its own
    /// default transport, for the same two reasons rmcp documents: idle
    /// connection pooling is disabled so an aborted response body cannot
    /// leave a half-consumed connection in the pool (and so a terminated
    /// request's socket closes at once, which is what a server observes as
    /// the cancellation), and redirects are disabled so configured headers —
    /// credentials included — can never be replayed to a redirect target.
    /// # Errors
    ///
    /// Returns an error when the HTTP client cannot be built. This is a
    /// connect-path failure, not a panic: `connect` is reached by ordinary
    /// reconnection, and a bounded reconnect attempt that cannot build a
    /// client must fail that one dispatch rather than take the runtime down.
    pub(crate) fn new() -> Result<(Self, Arc<McpHttpRequestOwnership>), crate::tools::mcp::McpError>
    {
        let ownership = Arc::new(McpHttpRequestOwnership::default());
        let inner = reqwest::Client::builder()
            .pool_max_idle_per_host(0)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|error| {
                crate::tools::mcp::McpError::Configuration(format!(
                    "the MCP HTTP client could not be built: {error}"
                ))
            })?;
        Ok((
            Self {
                inner,
                ownership: Arc::clone(&ownership),
            },
            ownership,
        ))
    }
}

/// The JSON-RPC request id one outbound message correlates to, when it has
/// one.
fn request_id(message: &ClientJsonRpcMessage) -> Option<RequestId> {
    match message {
        ClientJsonRpcMessage::Request(request) => Some(request.id.clone()),
        _ => None,
    }
}

/// The transport error one locally terminated request reports.
///
/// It is deliberately an ordinary transport-send failure: rmcp resolves the
/// request's local responder from it, so the dispatching call observes its
/// own termination through the same path any other send failure takes. The
/// MCP executor knows it terminated the request and therefore does not read
/// this as evidence about the connection generation's health.
fn locally_terminated() -> StreamableHttpError<reqwest::Error> {
    StreamableHttpError::Io(std::io::Error::other(
        "the MCP HTTP request was terminated by rustX because its tool call was cancelled",
    ))
}

impl McpHttpClient {
    /// Runs one outbound POST under rustX's local request ownership.
    ///
    /// Requests without a JSON-RPC id (notifications and responses) own no
    /// correlated local state a settlement could ever need to terminate, so
    /// they pass straight through.
    async fn owned_post(
        &self,
        message: &ClientJsonRpcMessage,
        post: impl Future<
            Output = Result<StreamableHttpPostResponse, StreamableHttpError<reqwest::Error>>,
        >,
    ) -> Result<StreamableHttpPostResponse, StreamableHttpError<reqwest::Error>> {
        let Some(id) = request_id(message) else {
            return post.await;
        };
        let guard = self.ownership.register(id);
        let terminate = guard.terminate.clone();
        let response = tokio::select! {
            biased;
            // Local termination drops the POST future here, inside this
            // scope, before the guard below is released.
            () = terminate.cancelled() => Err(locally_terminated()),
            response = post => response,
        };
        // An SSE response keeps the HTTP response body open past this
        // function, so ownership travels into the stream rather than ending
        // here. Every other outcome leaves no local HTTP activity, and
        // dropping the guard at the end of this scope releases the latch.
        match response {
            Ok(StreamableHttpPostResponse::Sse(stream, session)) => {
                Ok(StreamableHttpPostResponse::Sse(
                    OwnedResponseStream {
                        inner: Some(stream),
                        terminated: Box::pin(terminate.cancelled_owned()),
                        guard: Some(guard),
                    }
                    .boxed(),
                    session,
                ))
            }
            other => other,
        }
    }
}

impl StreamableHttpClient for McpHttpClient {
    type Error = reqwest::Error;

    async fn post_message(
        &self,
        uri: Arc<str>,
        message: ClientJsonRpcMessage,
        session_id: Option<Arc<str>>,
        auth_header: Option<String>,
        custom_headers: CustomHeaders,
    ) -> Result<StreamableHttpPostResponse, StreamableHttpError<Self::Error>> {
        self.owned_post(
            &message,
            self.inner.post_message(
                uri,
                message.clone(),
                session_id,
                auth_header,
                custom_headers,
            ),
        )
        .await
    }

    async fn post_message_with_max_sse_event_size(
        &self,
        uri: Arc<str>,
        message: ClientJsonRpcMessage,
        session_id: Option<Arc<str>>,
        auth_header: Option<String>,
        custom_headers: CustomHeaders,
        max_sse_event_size: usize,
    ) -> Result<StreamableHttpPostResponse, StreamableHttpError<Self::Error>> {
        self.owned_post(
            &message,
            self.inner.post_message_with_max_sse_event_size(
                uri,
                message.clone(),
                session_id,
                auth_header,
                custom_headers,
                max_sse_event_size,
            ),
        )
        .await
    }

    async fn delete_session(
        &self,
        uri: Arc<str>,
        session_id: Arc<str>,
        auth_header: Option<String>,
        custom_headers: CustomHeaders,
    ) -> Result<(), StreamableHttpError<Self::Error>> {
        self.inner
            .delete_session(uri, session_id, auth_header, custom_headers)
            .await
    }

    async fn get_stream(
        &self,
        uri: Arc<str>,
        session_id: Option<Arc<str>>,
        last_event_id: Option<String>,
        auth_header: Option<String>,
        custom_headers: CustomHeaders,
    ) -> Result<SseStream, StreamableHttpError<Self::Error>> {
        self.inner
            .get_stream(uri, session_id, last_event_id, auth_header, custom_headers)
            .await
    }

    async fn get_stream_with_max_sse_event_size(
        &self,
        uri: Arc<str>,
        session_id: Option<Arc<str>>,
        last_event_id: Option<String>,
        auth_header: Option<String>,
        custom_headers: CustomHeaders,
        max_sse_event_size: usize,
    ) -> Result<SseStream, StreamableHttpError<Self::Error>> {
        // The server-initiated stream is not request-scoped: no JSON-RPC
        // request id owns it, so no settlement ever terminates it and there
        // is nothing here for this layer to own. It ends with the transport.
        self.inner
            .get_stream_with_max_sse_event_size(
                uri,
                session_id,
                last_event_id,
                auth_header,
                custom_headers,
                max_sse_event_size,
            )
            .await
    }
}

/// Deterministic regressions for the local request ownership contract
/// (Issue #205 review finding 1).
///
/// These pin the two facts the MCP settlement plane relies on, with no
/// transport, no server, and no timer: terminating a request cancels the
/// token its in-flight POST selects on, and the release proof resolves only
/// **after** the guard that owns that request has been dropped.
#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use rmcp::model::RequestId;

    use super::{LocalRequestTermination, MAX_PRE_TERMINATED_REQUESTS, McpHttpRequestOwnership};

    fn id(value: i64) -> RequestId {
        RequestId::Number(value)
    }

    /// The release proof is a proof: it must not resolve while the request's
    /// ownership guard — held by its POST future, or by the response body
    /// that future produced — is still alive.
    #[tokio::test]
    async fn the_release_proof_resolves_only_after_the_owning_guard_is_dropped() {
        let ownership = Arc::new(McpHttpRequestOwnership::default());
        let guard = ownership.register(id(1));
        let termination = ownership.terminate(&id(1));
        assert!(
            termination.terminated_local_request(),
            "a live local request was found and terminated"
        );
        assert!(
            guard.terminate.is_cancelled(),
            "termination cancels the token the in-flight POST selects on"
        );
        // The guard still owns the request, so settlement must not proceed.
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(50), termination.settled())
                .await
                .is_err(),
            "the proof does not resolve while the request is still owned"
        );
        drop(guard);
        tokio::time::timeout(std::time::Duration::from_secs(5), termination.settled())
            .await
            .expect("dropping the owning guard releases the proof");
    }

    /// A request terminated before its POST started must never reach the
    /// network afterwards: the ledger pre-terminates the registration that
    /// arrives later. This is the dispatch/cancellation race, closed without
    /// a timer.
    #[tokio::test]
    async fn a_request_terminated_before_its_post_started_is_pre_terminated() {
        let ownership = Arc::new(McpHttpRequestOwnership::default());
        let termination = ownership.terminate(&id(7));
        assert!(
            matches!(termination, LocalRequestTermination::Settled),
            "nothing was in flight, so nothing rustX owns is still running"
        );
        termination.settled().await;
        // The POST starts only now.
        let guard = ownership.register(id(7));
        assert!(
            guard.terminate.is_cancelled(),
            "the POST is pre-terminated and never reaches the network"
        );
    }

    /// An unrelated request is untouched by another's termination, and the
    /// ledger is bounded.
    #[tokio::test]
    async fn termination_is_per_request_and_the_ledger_is_bounded() {
        let ownership = Arc::new(McpHttpRequestOwnership::default());
        let kept = ownership.register(id(1));
        let doomed = ownership.register(id(2));
        ownership.terminate(&id(2));
        assert!(doomed.terminate.is_cancelled());
        assert!(
            !kept.terminate.is_cancelled(),
            "only the named request ends"
        );

        for value in 100..(100 + i64::try_from(MAX_PRE_TERMINATED_REQUESTS).expect("small bound")) {
            ownership.terminate(&id(value));
        }
        // One past the bound: the oldest ledger entry is evicted, the newest
        // still pre-terminates its POST.
        let overflow = 100 + i64::try_from(MAX_PRE_TERMINATED_REQUESTS).expect("small bound");
        ownership.terminate(&id(overflow));
        assert!(ownership.register(id(overflow)).terminate.is_cancelled());
        assert!(!ownership.register(id(100)).terminate.is_cancelled());
    }

    /// Close terminates every request the generation still owns, so drain
    /// inherits no live per-request control primitive.
    #[tokio::test]
    async fn terminate_all_ends_every_request_the_generation_still_owns() {
        let ownership = Arc::new(McpHttpRequestOwnership::default());
        let guards: Vec<_> = (1..=5).map(|value| ownership.register(id(value))).collect();
        ownership.terminate_all();
        assert!(guards.iter().all(|guard| guard.terminate.is_cancelled()));
    }
}
