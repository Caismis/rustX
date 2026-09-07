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
//!              response                       request's lifecycle entry
//! ```
//!
//! # The request lifecycle
//!
//! [`McpHttpRequestOwnership`] is the per-connection-generation registry of
//! the tool-invocation requests this generation is responsible for. Each one
//! has **one** lifecycle entry, keyed by the exact JSON-RPC
//! [`RequestId`] rmcp put on the wire — never a parallel correlation id —
//! and that entry, not the *absence* of an entry, is what a cancellation
//! reads:
//!
//! ```text
//!                     admit(id) / begin_dispatch(id)
//!                                  |
//!                                  v
//!                        +---------------------+
//!                        |  NotYetRegistered   |   nothing local has begun
//!                        +---------------------+
//!                                  |  Transport::send takes dispatch
//!                                  |  ownership, then McpHttpClient's POST
//!                                  |  takes HTTP ownership
//!                                  v
//!                        +---------------------+
//!                        |        Live         |   >= 1 local owner
//!                        +---------------------+
//!                                  |  every local owner dropped
//!                                  |  (POST future, then SSE body)
//!                                  v
//!                        +---------------------+
//!                        |      Released       |   no local activity remains
//!                        +---------------------+
//!                                  |  the invocation's admission guard drops
//!                                  v
//!                             (forgotten)
//! ```
//!
//! **Absence never means "not yet registered".** The previous shape inferred
//! the pre-registration state from a request id being missing from the live
//! map, which is also what a *completed* request looks like: a cancellation
//! landing after the POST released but before rmcp delivered the correlated
//! response created a pre-termination record for a request that could never
//! register again, and that record then survived until the connection
//! generation closed. An explicit `Released` state answers that question
//! instead of guessing it, and creates nothing.
//!
//! # The two local owners
//!
//! A request's local activity has two owners, and "no local activity
//! remains" means both are gone:
//!
//! - **dispatch ownership** is taken synchronously inside `Transport::send`
//!   ([`crate::tools::mcp::dispatch`]) and held by the future that carries
//!   the message to rmcp's transport worker. It covers the window in which
//!   the POST has not started but is still going to;
//! - **HTTP ownership** is taken by [`McpHttpClient`] — the
//!   [`StreamableHttpClient`] rmcp actually posts through — and held for
//!   exactly as long as any HTTP activity of that request exists: the POST
//!   future while it awaits response headers, and then the SSE response body
//!   stream, which the guard travels into.
//!
//! Terminating one request cancels its token. The wrapper reacts by
//! **dropping the inner POST future or the inner response body first, and
//! releasing its ownership only afterwards**, so awaiting the release latch
//! is a real ownership proof rather than a restatement of "we stopped
//! waiting". A request terminated while it is still `NotYetRegistered` never
//! reaches the network at all: `Transport::send` refuses it before it is
//! handed to rmcp's transport, and any POST that somehow still starts finds
//! the token already cancelled.
//!
//! # Boundedness
//!
//! One small entry per tool-invocation request this generation has admitted
//! and not yet forgotten — that is, `O(in-flight tool calls)`, never
//! `O(requests ever raced)`. Every entry has a request-local forget point
//! (its invocation's admission guard), so normal completion cleans up its
//! own state without waiting for the connection to close; close only clears
//! whatever is still genuinely in flight. No task is spawned, nothing is
//! retried, and nothing survives the connection generation.

use std::collections::HashMap;
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

/// A one-shot latch that resolves once its request's local ownership has
/// been released.
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

/// One tool-invocation request's local lifecycle inside one connection
/// generation.
///
/// The state is derived from the two ownership flags rather than stored
/// twice: see [`RequestLifecycle::state`].
struct RequestLifecycle {
    /// Cancels every local activity of this request. It exists from the
    /// entry's creation, so a termination that arrives before any local
    /// activity exists still binds every activity that starts afterwards.
    terminate: CancellationToken,
    /// Resolves once no rustX-owned local activity of this request remains.
    release: Arc<ReleaseLatch>,
    /// Which local owner, if any, holds this request's physical activity.
    owner: LocalOwner,
    /// An MCP invocation still holds this entry. Its admission guard is the
    /// request-local forget point.
    admitted: bool,
    /// The outbound dispatch seam created this entry and the invocation's
    /// admission has not arrived yet. It always does — the executor admits
    /// its request id with no await between the effect frontier and the
    /// admission — so this is only ever open across one interleaving.
    awaiting_admission: bool,
}

/// Which local object currently owns one request's physical activity.
///
/// Exactly one at a time, because ownership is a **baton**: the outbound
/// dispatch hands it to the POST the moment the POST registers. Holding both
/// would make a cancellation's local settlement wait for rmcp to resolve an
/// outbound send, which is precisely the dependency the Streamable HTTP
/// contract removes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LocalOwner {
    /// No local activity of this request has begun.
    None,
    /// The outbound dispatch future (`Transport::send`) owns it: the request
    /// has left the peer's outbound queue and its POST has not started.
    Dispatch,
    /// An HTTP request guard owns it: the POST future while it awaits
    /// response headers, and then the SSE response body it produced.
    Http,
    /// Every local owner has been and gone.
    Released,
}

/// The explicit state of one request's local lifecycle.
///
/// It is what a cancellation reads. Deriving it from [`LocalOwner`] keeps one
/// source of truth: there is no way to be `Released` and still own the
/// response body, and no way to be `NotYetRegistered` after any local owner
/// has existed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RequestState {
    /// Admitted; no local activity of this request has begun.
    NotYetRegistered,
    /// A local owner exists: the outbound dispatch, the POST future, or the
    /// response body stream.
    Live,
    /// Every local owner has been dropped. No local activity remains and,
    /// because the entry still exists, this is *known* rather than inferred
    /// from an absent record.
    Released,
}

impl RequestLifecycle {
    fn new(terminate: CancellationToken) -> Self {
        Self {
            terminate,
            release: Arc::new(ReleaseLatch::default()),
            owner: LocalOwner::None,
            admitted: false,
            awaiting_admission: false,
        }
    }

    const fn state(&self) -> RequestState {
        match self.owner {
            LocalOwner::None => RequestState::NotYetRegistered,
            LocalOwner::Dispatch | LocalOwner::Http => RequestState::Live,
            LocalOwner::Released => RequestState::Released,
        }
    }

    /// Whether this entry has reached its own terminal forget point: no
    /// local owner remains and no invocation still holds it.
    const fn forgettable(&self) -> bool {
        !matches!(self.owner, LocalOwner::Dispatch | LocalOwner::Http)
            && !self.admitted
            && !self.awaiting_admission
    }
}

#[derive(Default)]
struct OwnershipState {
    /// One entry per tool-invocation request of this generation that has
    /// been admitted and not yet forgotten.
    requests: HashMap<RequestId, RequestLifecycle>,
}

/// The rustX-owned local request ownership of one Streamable HTTP
/// connection generation.
#[derive(Default)]
pub(crate) struct McpHttpRequestOwnership {
    state: Mutex<OwnershipState>,
}

/// What terminating one request's local ownership found, and the proof that
/// it is settled.
///
/// The four cases carry genuinely different settlement evidence, so they are
/// not collapsed: `settled()` is the executor's local settlement bound, and
/// it depends on rustX-owned state only — no remote response, no protocol
/// acknowledgement, and no timer participates in it.
pub(crate) enum LocalRequestTermination {
    /// This transport owns no per-request local state at all. Over stdio an
    /// outbound write owns no resource that outlives it, so there is no
    /// local half of the request to terminate or to prove released.
    NoLocalOwnership,
    /// No local activity of this request had begun, and none can begin: the
    /// request's token is cancelled, so the outbound dispatch seam refuses
    /// to hand it to rmcp's transport and any POST that still starts is
    /// pre-terminated before it can reach the network.
    PreDispatchTerminated,
    /// Every local owner of this request had already been dropped before the
    /// termination. The local half finished on its own; nothing was
    /// terminated and nothing is pending. **No record is created**: this
    /// request can never register again.
    AlreadyReleased,
    /// A local owner existed and was terminated. The latch resolves once
    /// that owner — the outbound dispatch future, the POST future, or the
    /// response body it produced — has actually been dropped.
    TerminatedPendingRelease(Arc<ReleaseLatch>),
}

impl LocalRequestTermination {
    /// Whether *this call* terminated its own transport-level request.
    ///
    /// A call that did explains the transport-send failure its dispatch then
    /// observes, so that failure is not independent evidence that the
    /// connection generation died. `AlreadyReleased` deliberately does not
    /// qualify: nothing was terminated there, so a transport failure is
    /// still the transport's own fact.
    pub(crate) const fn terminated_local_request(&self) -> bool {
        matches!(
            self,
            Self::PreDispatchTerminated | Self::TerminatedPendingRelease(_)
        )
    }

    /// Whether [`Self::settled`] is a real event rather than an
    /// already-settled fact.
    ///
    /// Only a termination that is waiting for a local owner to be dropped
    /// can settle *later* than the caller, so only that case may be raced
    /// against the best-effort remote cancellation send.
    pub(crate) const fn awaits_release(&self) -> bool {
        matches!(self, Self::TerminatedPendingRelease(_))
    }

    /// Awaits the proof that no rustX-owned local activity of this request
    /// remains.
    pub(crate) async fn settled(&self) {
        match self {
            Self::NoLocalOwnership | Self::PreDispatchTerminated | Self::AlreadyReleased => (),
            Self::TerminatedPendingRelease(latch) => latch.released().await,
        }
    }
}

/// One MCP invocation's hold on its request's lifecycle entry.
///
/// Created at the invocation's own admission — the instant after its effect
/// frontier, where the request id first exists — and dropped when the
/// invocation ends, whatever its outcome. That drop is the request-local
/// forget point: it is why normal completion cleans up its own state instead
/// of leaving one entry per historical request behind until the connection
/// generation closes.
pub(crate) struct McpRequestAdmission {
    ownership: Arc<McpHttpRequestOwnership>,
    id: RequestId,
}

impl McpRequestAdmission {
    /// Terminates the local ownership of this admitted request.
    ///
    /// Synchronous and unconditional: from the moment it returns, this
    /// request cannot reach the network even if no POST has started yet.
    pub(crate) fn terminate(&self) -> LocalRequestTermination {
        self.ownership.terminate(&self.id)
    }
}

impl Drop for McpRequestAdmission {
    fn drop(&mut self) {
        self.ownership.release_admission(&self.id);
    }
}

/// The outbound dispatch's hold on one request's lifecycle entry.
///
/// Held by the future that carries the message to rmcp's transport worker,
/// so the window in which a POST has not started but is still going to is
/// owned rather than guessed.
pub(crate) struct DispatchOwnership {
    /// `None` over a transport with no per-request lifecycle — stdio, where
    /// an outbound write owns no resource that outlives it.
    ownership: Option<Arc<McpHttpRequestOwnership>>,
    id: Option<RequestId>,
}

impl DispatchOwnership {
    /// The ownership of a transport that has no per-request lifecycle.
    pub(crate) const fn none() -> Self {
        Self {
            ownership: None,
            id: None,
        }
    }
}

impl Drop for DispatchOwnership {
    fn drop(&mut self) {
        if let (Some(ownership), Some(id)) = (&self.ownership, &self.id) {
            ownership.release_dispatch(id);
        }
    }
}

/// What the outbound dispatch seam may do with one request.
pub(crate) enum OutboundDispatch {
    /// The request may go to the transport; the guard owns it until the
    /// outbound send future resolves or is dropped.
    Owned(DispatchOwnership),
    /// The request was terminated before it was dispatched, so it must never
    /// be handed to the transport at all.
    Refused,
}

impl McpHttpRequestOwnership {
    /// Admits one dispatched tool-invocation request.
    ///
    /// Called by the MCP executor at the instant after its effect frontier,
    /// which is the first moment the request id exists. The outbound
    /// dispatch seam may reach the same id first — the peer's outbound queue
    /// and the dispatching task run concurrently — so this is a
    /// get-or-create, and either order produces the same single entry.
    pub(crate) fn admit(self: &Arc<Self>, id: &RequestId) -> McpRequestAdmission {
        {
            let mut state = self.state.lock().expect("MCP HTTP ownership lock poisoned");
            let entry = state
                .requests
                .entry(id.clone())
                .or_insert_with(|| RequestLifecycle::new(CancellationToken::new()));
            entry.admitted = true;
            entry.awaiting_admission = false;
        }
        McpRequestAdmission {
            ownership: Arc::clone(self),
            id: id.clone(),
        }
    }

    /// Takes dispatch ownership of one outbound tool-invocation request, or
    /// refuses it because it has already been terminated.
    ///
    /// Called synchronously inside `Transport::send`, before the message can
    /// be handed to rmcp's transport. Refusing here is what makes
    /// [`LocalRequestTermination::PreDispatchTerminated`] honest: the
    /// request never reaches the network, and no local activity of it will
    /// ever be created.
    ///
    /// The ownership this takes covers exactly one window — the request has
    /// left the peer's outbound queue and its POST has not registered yet —
    /// and is handed to the HTTP guard the moment it does.
    pub(crate) fn begin_dispatch(self: &Arc<Self>, id: &RequestId) -> OutboundDispatch {
        let mut state = self.state.lock().expect("MCP HTTP ownership lock poisoned");
        let entry = state.requests.entry(id.clone()).or_insert_with(|| {
            let mut fresh = RequestLifecycle::new(CancellationToken::new());
            fresh.awaiting_admission = true;
            fresh
        });
        if entry.terminate.is_cancelled() {
            return OutboundDispatch::Refused;
        }
        if !matches!(entry.owner, LocalOwner::None) {
            // Fail closed. One request id is dispatched exactly once per
            // connection generation, so a second outbound send for the same
            // id is either a duplicate or a replay; neither may silently
            // overwrite the ownership state of the request already using it.
            return OutboundDispatch::Refused;
        }
        entry.owner = LocalOwner::Dispatch;
        drop(state);
        OutboundDispatch::Owned(DispatchOwnership {
            ownership: Some(Arc::clone(self)),
            id: Some(id.clone()),
        })
    }

    /// Registers one request's in-flight HTTP activity and returns the guard
    /// that owns it.
    ///
    /// The guard's token is already cancelled when the request was
    /// terminated before its POST started, so the POST is pre-terminated
    /// instead of reaching the network. A request with no lifecycle entry —
    /// every request rustX sends that is not a tool invocation — owns no
    /// state a settlement could terminate and is registered as untracked.
    fn register(self: &Arc<Self>, id: RequestId) -> RequestOwnershipGuard {
        let mut state = self.state.lock().expect("MCP HTTP ownership lock poisoned");
        let Some(entry) = state.requests.get_mut(&id) else {
            // Not a tracked tool invocation: nothing terminates it, and it
            // must not create a lifecycle entry nothing would forget.
            return RequestOwnershipGuard {
                ownership: None,
                id,
                terminate: CancellationToken::new(),
            };
        };
        if matches!(entry.owner, LocalOwner::Http) {
            // Fail closed: a live HTTP ownership for this id already exists
            // and is not overwritten. The duplicate registration is given a
            // pre-cancelled, untracked guard, so it cannot reach the network
            // and cannot release someone else's ownership.
            let refused = CancellationToken::new();
            refused.cancel();
            return RequestOwnershipGuard {
                ownership: None,
                id,
                terminate: refused,
            };
        }
        // The baton passes here: the POST future and the response body are
        // the request's real local activity from now on.
        entry.owner = LocalOwner::Http;
        let terminate = entry.terminate.clone();
        drop(state);
        RequestOwnershipGuard {
            ownership: Some(Arc::clone(self)),
            id,
            terminate,
        }
    }

    /// Terminates the local ownership of one request, reading its explicit
    /// state rather than inferring one from an absent record.
    fn terminate(&self, id: &RequestId) -> LocalRequestTermination {
        let mut state = self.state.lock().expect("MCP HTTP ownership lock poisoned");
        let Some(entry) = state.requests.get_mut(id) else {
            // Only an admitted request can be terminated, and an admission
            // holds its entry for the whole invocation, so this is
            // unreachable from the executor. Reporting "no local ownership"
            // is the fail-safe direction: it claims no termination happened
            // and creates no record that could outlive the request.
            return LocalRequestTermination::NoLocalOwnership;
        };
        entry.terminate.cancel();
        match entry.state() {
            RequestState::NotYetRegistered => LocalRequestTermination::PreDispatchTerminated,
            RequestState::Released => LocalRequestTermination::AlreadyReleased,
            RequestState::Live => {
                LocalRequestTermination::TerminatedPendingRelease(Arc::clone(&entry.release))
            }
        }
    }

    /// Terminates every request this generation still owns locally.
    ///
    /// Connection close calls this before it awaits rmcp's transport
    /// shutdown, so drain owns the per-request control primitives introduced
    /// here rather than inheriting them.
    pub(crate) fn terminate_all(&self) {
        let mut state = self.state.lock().expect("MCP HTTP ownership lock poisoned");
        for entry in state.requests.values_mut() {
            entry.terminate.cancel();
            // After close no POST of this generation can start, so an entry
            // that only existed to bind a dispatch that will never happen
            // has nothing left to protect.
            entry.awaiting_admission = false;
        }
        state.requests.retain(|_, entry| !entry.forgettable());
    }

    /// Releases one request's HTTP ownership, but only if the entry is still
    /// the one this guard registered.
    fn release_http(&self, id: &RequestId) {
        let mut state = self.state.lock().expect("MCP HTTP ownership lock poisoned");
        let Some(entry) = state.requests.get_mut(id) else {
            return;
        };
        if matches!(entry.owner, LocalOwner::Http) {
            entry.owner = LocalOwner::Released;
        }
        Self::settle_entry(&mut state, id);
    }

    /// Releases one request's dispatch ownership.
    ///
    /// A no-op once the POST has taken the baton: from that point the HTTP
    /// guard is what owns the request, and the outbound send resolving says
    /// nothing about it.
    fn release_dispatch(&self, id: &RequestId) {
        let mut state = self.state.lock().expect("MCP HTTP ownership lock poisoned");
        let Some(entry) = state.requests.get_mut(id) else {
            return;
        };
        if matches!(entry.owner, LocalOwner::Dispatch) {
            entry.owner = LocalOwner::Released;
        }
        Self::settle_entry(&mut state, id);
    }

    /// Releases one invocation's admission, which is the request-local
    /// forget point.
    fn release_admission(&self, id: &RequestId) {
        let mut state = self.state.lock().expect("MCP HTTP ownership lock poisoned");
        let Some(entry) = state.requests.get_mut(id) else {
            return;
        };
        entry.admitted = false;
        Self::settle_entry(&mut state, id);
    }

    /// Fires the release proof once no local owner remains, and forgets the
    /// entry once nothing holds it at all.
    ///
    /// The latch is released *after* the caller has already dropped the
    /// object that owned the request — the POST future, the response body,
    /// or the outbound send future — so awaiting it is ownership evidence
    /// rather than a restatement of "we stopped waiting".
    fn settle_entry(state: &mut OwnershipState, id: &RequestId) {
        let Some(entry) = state.requests.get_mut(id) else {
            return;
        };
        if matches!(entry.owner, LocalOwner::Released) {
            entry.release.release();
        }
        if entry.forgettable() {
            state.requests.remove(id);
        }
    }

    /// How many request lifecycle entries this generation currently holds.
    ///
    /// The memory bound of this layer, exposed so a regression can assert it
    /// directly instead of inferring it.
    #[cfg(test)]
    pub(crate) fn outstanding_requests(&self) -> usize {
        self.state
            .lock()
            .expect("MCP HTTP ownership lock poisoned")
            .requests
            .len()
    }

    /// The explicit lifecycle state of one request, when it is still known.
    #[cfg(test)]
    fn state_of(&self, id: &RequestId) -> Option<RequestState> {
        self.state
            .lock()
            .expect("MCP HTTP ownership lock poisoned")
            .requests
            .get(id)
            .map(RequestLifecycle::state)
    }
}

/// The ownership of one request's local HTTP activity.
///
/// Dropping it is what releases the request's HTTP ownership, so it is held
/// by whichever local object is still alive: the POST future, and then the
/// response body stream it produced.
///
/// `ownership` is `None` for a request this generation tracks no lifecycle
/// for — anything that is not a tool invocation, and a duplicate
/// registration that was refused — so such a guard releases nothing and can
/// never disturb another request's state.
struct RequestOwnershipGuard {
    ownership: Option<Arc<McpHttpRequestOwnership>>,
    id: RequestId,
    terminate: CancellationToken,
}

impl Drop for RequestOwnershipGuard {
    fn drop(&mut self) {
        if let Some(ownership) = &self.ownership {
            ownership.release_http(&self.id);
        }
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

/// Deterministic regressions for the request lifecycle contract
/// (Issue #205).
///
/// These pin the state machine itself, with no transport, no server, and no
/// timer: every transition is driven explicitly, so "the POST has not
/// registered yet" and "the POST already finished" are two different proven
/// states rather than two readings of the same missing record.
#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use rmcp::model::RequestId;

    use super::{LocalRequestTermination, McpHttpRequestOwnership, OutboundDispatch, RequestState};

    fn id(value: i64) -> RequestId {
        RequestId::Number(value)
    }

    /// Takes dispatch ownership, asserting it was not refused.
    fn dispatch(ownership: &Arc<McpHttpRequestOwnership>, value: i64) -> super::DispatchOwnership {
        match ownership.begin_dispatch(&id(value)) {
            OutboundDispatch::Owned(guard) => guard,
            OutboundDispatch::Refused => panic!("request {value} must be dispatchable"),
        }
    }

    /// The release proof is a proof: it must not resolve while any local
    /// owner of the request — the outbound dispatch future, the POST future,
    /// or the response body that future produced — is still alive.
    #[tokio::test]
    async fn the_release_proof_resolves_only_after_every_local_owner_is_dropped() {
        let ownership = Arc::new(McpHttpRequestOwnership::default());
        let admission = ownership.admit(&id(1));
        let dispatching = dispatch(&ownership, 1);
        // While only the outbound dispatch owns the request, that is the
        // owner a termination has to wait for.
        let pre_registration = admission.terminate();
        assert!(pre_registration.awaits_release());
        assert!(
            tokio::time::timeout(
                std::time::Duration::from_millis(50),
                pre_registration.settled()
            )
            .await
            .is_err(),
            "the proof does not resolve while the outbound dispatch owns the request"
        );
        let guard = ownership.register(id(1));
        assert!(
            guard.terminate.is_cancelled(),
            "the POST inherits the termination and is pre-terminated"
        );
        // The baton passed to the POST, so the outbound dispatch future
        // resolving is no longer what settlement waits for.
        drop(dispatching);
        assert!(
            tokio::time::timeout(
                std::time::Duration::from_millis(50),
                pre_registration.settled()
            )
            .await
            .is_err(),
            "the proof does not resolve while the POST owns the request"
        );
        drop(guard);
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            pre_registration.settled(),
        )
        .await
        .expect("dropping the owning guard releases the proof");
        drop(admission);
        assert_eq!(ownership.outstanding_requests(), 0);
    }

    /// **Cancellation before POST registration.** The request is admitted,
    /// registration has not happened, and the cancellation wins.
    ///
    /// The settlement is honest without waiting for anything, because
    /// nothing local exists *and nothing local can come to exist*: the
    /// outbound dispatch seam refuses the request, so it is never handed to
    /// the transport and never reaches the network. The entry is forgotten
    /// at the invocation's own forget point.
    #[tokio::test]
    async fn cancellation_before_registration_refuses_the_dispatch_that_follows() {
        let ownership = Arc::new(McpHttpRequestOwnership::default());
        let admission = ownership.admit(&id(7));
        assert_eq!(
            ownership.state_of(&id(7)),
            Some(RequestState::NotYetRegistered)
        );
        let termination = admission.terminate();
        assert!(
            matches!(termination, LocalRequestTermination::PreDispatchTerminated),
            "nothing local had begun and nothing local can begin"
        );
        assert!(termination.terminated_local_request());
        assert!(!termination.awaits_release(), "there is nothing to await");
        termination.settled().await;
        // The outbound send is only reached now — and is refused, so the
        // request never leaves rustX.
        assert!(
            matches!(ownership.begin_dispatch(&id(7)), OutboundDispatch::Refused),
            "a terminated request is never handed to the transport"
        );
        // Any POST that somehow still started would be pre-terminated.
        assert!(
            ownership.register(id(7)).terminate.is_cancelled(),
            "the POST is pre-terminated and never reaches the network"
        );
        drop(admission);
        assert_eq!(
            ownership.outstanding_requests(),
            0,
            "the invocation's own forget point clears the entry"
        );
    }

    /// **Cancellation while live.** A POST owns the request; cancellation
    /// terminates that exact request and settlement waits for the release
    /// proof, never for a remote answer.
    #[tokio::test]
    async fn cancellation_while_live_terminates_that_exact_request() {
        let ownership = Arc::new(McpHttpRequestOwnership::default());
        let admission = ownership.admit(&id(2));
        let dispatching = dispatch(&ownership, 2);
        let guard = ownership.register(id(2));
        assert_eq!(ownership.state_of(&id(2)), Some(RequestState::Live));
        let termination = admission.terminate();
        assert!(
            termination.awaits_release(),
            "a live local owner must be proven released before settlement"
        );
        // The outbound send future resolving is deliberately not part of the
        // proof: settlement must never depend on rmcp servicing a send.
        drop(dispatching);
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(50), termination.settled())
                .await
                .is_err(),
            "the POST still owns this request"
        );
        drop(guard);
        tokio::time::timeout(std::time::Duration::from_secs(5), termination.settled())
            .await
            .expect("the release proof resolves");
        assert_eq!(ownership.state_of(&id(2)), Some(RequestState::Released));
    }

    /// **The response released before `handle.rx` delivery.** This is the
    /// race the previous shape could not see.
    ///
    /// Every local owner of the request has been dropped — the POST finished
    /// and its body is gone — but rmcp has not delivered the correlated
    /// response into the executor's channel yet, so a deadline or
    /// cancellation can still land here. Under the old shape the request id
    /// was simply absent from the live map, which was read as "the POST has
    /// not started yet", and a pre-termination record was created for a
    /// request that could never register again and then survived until the
    /// connection generation closed.
    #[tokio::test]
    async fn cancellation_after_release_creates_no_record_and_leaks_nothing() {
        let ownership = Arc::new(McpHttpRequestOwnership::default());
        let admission = ownership.admit(&id(3));
        let dispatching = dispatch(&ownership, 3);
        let guard = ownership.register(id(3));
        // The HTTP exchange completes normally: body consumed, POST done.
        // The outbound send future is still pending — rmcp has not resolved
        // it yet — and that deliberately does not hold the request open.
        drop(guard);
        assert_eq!(
            ownership.state_of(&id(3)),
            Some(RequestState::Released),
            "the completed request is known to be released, not inferred to be unstarted"
        );
        // The correlated response has not reached the executor yet; the
        // deadline fires here.
        let termination = admission.terminate();
        assert!(
            matches!(termination, LocalRequestTermination::AlreadyReleased),
            "the local half is already settled, so nothing was terminated"
        );
        assert!(
            !termination.terminated_local_request(),
            "nothing was terminated, so a transport failure is still the transport's own fact"
        );
        termination.settled().await;
        // No future registration can occur, and no record was created to
        // stop one.
        assert!(
            matches!(ownership.begin_dispatch(&id(3)), OutboundDispatch::Refused),
            "a released request id is never dispatched again"
        );
        drop(admission);
        drop(dispatching);
        assert_eq!(
            ownership.outstanding_requests(),
            0,
            "the race leaves no request lifecycle state behind"
        );
    }

    /// **Normal successful requests clean up their own state.** No
    /// cancellation, no close: after many complete exchanges the registry is
    /// empty, so the memory bound is the in-flight request count and not the
    /// count of requests this generation ever served.
    #[tokio::test]
    async fn many_successful_requests_leave_an_empty_registry() {
        let ownership = Arc::new(McpHttpRequestOwnership::default());
        for value in 0..1_000 {
            let admission = ownership.admit(&id(value));
            let dispatching = dispatch(&ownership, value);
            let guard = ownership.register(id(value));
            drop(guard);
            drop(dispatching);
            drop(admission);
        }
        assert_eq!(
            ownership.outstanding_requests(),
            0,
            "every request forgets its own state at its own terminal point"
        );
    }

    /// **Many released-versus-cancellation races.** The state stays bounded
    /// by the requests actually in flight, whichever way each race lands.
    #[tokio::test]
    async fn many_cancellation_races_stay_bounded_by_the_in_flight_set() {
        let ownership = Arc::new(McpHttpRequestOwnership::default());
        for value in 0..2_000 {
            let admission = ownership.admit(&id(value));
            let dispatching = dispatch(&ownership, value);
            let guard = ownership.register(id(value));
            if value % 2 == 0 {
                // The release wins the race.
                drop(guard);
                drop(dispatching);
                assert!(matches!(
                    admission.terminate(),
                    LocalRequestTermination::AlreadyReleased
                ));
            } else {
                // The cancellation wins the race.
                let termination = admission.terminate();
                assert!(termination.awaits_release());
                drop(guard);
                drop(dispatching);
                termination.settled().await;
            }
            drop(admission);
            assert_eq!(
                ownership.outstanding_requests(),
                0,
                "request {value} left no state behind"
            );
        }
    }

    /// An unrelated request is untouched by another's termination.
    #[tokio::test]
    async fn termination_is_per_request() {
        let ownership = Arc::new(McpHttpRequestOwnership::default());
        let kept_admission = ownership.admit(&id(1));
        let doomed_admission = ownership.admit(&id(2));
        let _kept_dispatch = dispatch(&ownership, 1);
        let _doomed_dispatch = dispatch(&ownership, 2);
        let kept = ownership.register(id(1));
        let doomed = ownership.register(id(2));
        doomed_admission.terminate();
        assert!(doomed.terminate.is_cancelled());
        assert!(
            !kept.terminate.is_cancelled(),
            "only the named request ends"
        );
        drop(kept_admission);
    }

    /// Every outstanding pre-dispatch termination is honoured, however many
    /// are open at once. The count is not a bound: a forgotten record would
    /// let a POST reach the server *after* its tool call had already
    /// settled.
    #[tokio::test]
    async fn every_outstanding_pre_dispatch_termination_is_honoured() {
        let ownership = Arc::new(McpHttpRequestOwnership::default());
        let outstanding = 512_i64;
        let admissions: Vec<_> = (0..outstanding)
            .map(|value| ownership.admit(&id(value)))
            .collect();
        for admission in &admissions {
            assert!(matches!(
                admission.terminate(),
                LocalRequestTermination::PreDispatchTerminated
            ));
        }
        // Dispatch order is rmcp's, not settlement's, so the oldest
        // termination must hold as surely as the newest.
        for value in (0..outstanding).rev() {
            assert!(
                matches!(
                    ownership.begin_dispatch(&id(value)),
                    OutboundDispatch::Refused
                ),
                "request {value} was terminated before dispatch and must never reach \
                 the transport"
            );
        }
        drop(admissions);
        assert_eq!(ownership.outstanding_requests(), 0);
    }

    /// One request id owns one lifecycle entry per connection generation.
    /// A duplicate dispatch or a duplicate registration fails closed rather
    /// than overwriting the ownership state the live request is using.
    #[tokio::test]
    async fn duplicate_dispatch_and_registration_fail_closed() {
        let ownership = Arc::new(McpHttpRequestOwnership::default());
        let admission = ownership.admit(&id(5));
        let dispatching = dispatch(&ownership, 5);
        assert!(
            matches!(ownership.begin_dispatch(&id(5)), OutboundDispatch::Refused),
            "a second outbound send for a live request id is refused"
        );
        let live = ownership.register(id(5));
        drop(dispatching);
        let duplicate = ownership.register(id(5));
        assert!(
            duplicate.terminate.is_cancelled(),
            "the duplicate registration can never reach the network"
        );
        assert!(
            !live.terminate.is_cancelled(),
            "the live registration is untouched by the refused duplicate"
        );
        // The refused duplicate owns nothing, so dropping it releases
        // nothing that belongs to the live request.
        drop(duplicate);
        assert_eq!(ownership.state_of(&id(5)), Some(RequestState::Live));
        drop(live);
        assert_eq!(ownership.state_of(&id(5)), Some(RequestState::Released));
        drop(admission);
    }

    /// A request that is not a tool invocation has no lifecycle entry, so
    /// its POST registers untracked and creates nothing for anyone to
    /// forget.
    #[tokio::test]
    async fn an_untracked_request_creates_no_lifecycle_state() {
        let ownership = Arc::new(McpHttpRequestOwnership::default());
        let guard = ownership.register(id(11));
        assert!(!guard.terminate.is_cancelled());
        assert_eq!(ownership.outstanding_requests(), 0);
        drop(guard);
        assert_eq!(ownership.outstanding_requests(), 0);
    }

    /// Close terminates every request the generation still owns, so drain
    /// inherits no live per-request control primitive — and it clears the
    /// entries that only existed to bind a dispatch that can no longer
    /// happen.
    #[tokio::test]
    async fn close_terminates_and_clears_what_the_generation_still_owns() {
        let ownership = Arc::new(McpHttpRequestOwnership::default());
        let admissions: Vec<_> = (1..=5).map(|value| ownership.admit(&id(value))).collect();
        let dispatches: Vec<_> = (1..=5).map(|value| dispatch(&ownership, value)).collect();
        let guards: Vec<_> = (1..=5).map(|value| ownership.register(id(value))).collect();
        // A request the outbound seam opened whose invocation never arrived.
        assert!(matches!(
            ownership.begin_dispatch(&id(99)),
            OutboundDispatch::Owned(_)
        ));
        ownership.terminate_all();
        assert!(guards.iter().all(|guard| guard.terminate.is_cancelled()));
        drop(guards);
        drop(dispatches);
        drop(admissions);
        assert_eq!(
            ownership.outstanding_requests(),
            0,
            "close leaves no request lifecycle state alive"
        );
    }
}
