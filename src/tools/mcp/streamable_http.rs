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
//! # What this layer is *not*
//!
//! It is not a second Streamable HTTP implementation and must never become
//! one. Every protocol decision stays rmcp's: session semantics (and their
//! deliberate absence from 2026-07-28 under SEP-2567), SSE framing and
//! resumption, and the SEP-2243 routing metadata — `Mcp-Method`,
//! `Mcp-Name`, `Mcp-Param-*`, `Mcp-Protocol-Version` — that rmcp generates
//! per request and hands down as `custom_headers`. This client forwards
//! those headers verbatim to the inner reqwest client and synthesizes none
//! of its own; it introduces no session identity, no cache, and no
//! retry/reinit policy. The only thing it adds to an exchange is the
//! ownership registration of a request-carrying POST.
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
//!   admit(id)                    begin_dispatch(id)
//!   the executor, one statement   the outbound seam, inside
//!   after its effect frontier     `Transport::send`'s synchronous prologue
//!            \                   /
//!             v                 v      whichever arrives first creates it
//!         +-----------------------------+
//!         |      AwaitingDispatch        |  the request is on rmcp's peer
//!         +-----------------------------+  outbound queue; nothing local
//!             |                    |       has begun, and the outbound
//!             |                    |       participant has not arrived
//!             |                    |
//!   Transport::send prologue    the generation's outbound seam ends
//!   takes ownership             (`no_further_dispatch`): the participant
//!             |                  can never arrive
//!             v                    |
//!         +-----------------+       |
//!         |  DispatchOwned  |       |   the send future owns the request
//!         +-----------------+       |   and has not handed it to the
//!             |         |           |   inner transport, or is awaiting it
//!             |         |           |
//!   the POST  |         | the send refuses the request, or resolves,
//!   registers |         | or is dropped
//!             v         |           |
//!         +-------------+           |
//!         |  HttpOwned  |           |   the POST future, then the SSE
//!         +-------------+           |   response body it produced
//!             |                     |
//!   every HTTP owner dropped        |
//!             |                     |
//!             v                     v
//!         +--------------------------------+
//!         |            Released            |  no local participant of this
//!         +--------------------------------+  request can dispatch it or
//!                        |                    hold HTTP activity again
//!         the invocation's admission guard drops
//!                        v
//!                   (forgotten)
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
//! # The three local participants, and why the first one is not optional
//!
//! A request's local activity has three participants, and "no local activity
//! remains" means all three are terminal:
//!
//! - **the outbound dispatch participant.** `Peer::send_cancellable_request`
//!   returning `Ok` only enqueues the request on rmcp's peer channel. rmcp's
//!   service loop dequeues it later and calls [`Transport::send`], whose
//!   **synchronous prologue** is where
//!   [`crate::tools::mcp::dispatch`] takes dispatch ownership. Between those
//!   two instants the request has no local owner and is nevertheless still
//!   fully capable of reaching the network, so `AwaitingDispatch` is a real
//!   ownership state and not a gap. Treating it as "nothing pending" is what
//!   let a terminated invocation's entry be forgotten and then *recreated*
//!   by the very send that was supposed to observe the termination;
//! - **dispatch ownership** is held by the future `Transport::send` returned,
//!   from before it is first polled until it resolves, is dropped, or hands
//!   the baton to the POST;
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
//! waiting". A request terminated while it is still `AwaitingDispatch` or
//! `DispatchOwned` never reaches the network at all: the seam refuses it
//! before the message is handed to rmcp's transport, and that refusal — not
//! an assumption about it — is what releases the latch.
//!
//! # The terminal ordering contract
//!
//! > Terminal Tool settlement happens-after every local participant capable
//! > of later dispatching that `RequestId` has become terminal.
//!
//! The release latch is what makes that a fact rather than a hope. It fires
//! only when the entry reaches `Released`, and `Released` is reachable only
//! when the outbound participant has arrived and decided, or has been proven
//! unable to arrive because the generation's outbound seam is over.
//!
//! # One monotone lifecycle per request id
//!
//! > Request lifecycle authority is created once per `RequestId` per
//! > connection generation and may only move toward terminality. It is never
//! > resurrected.
//!
//! An entry is removed only once its phase is `Released` — every participant
//! terminal — *and* no invocation still holds it, so a late participant can
//! never find its own entry missing and create a fresh, uncancelled one in
//! its place. rmcp calls `Transport::send` exactly once per outbound request
//! id, which is the structural premise the create-on-first-arrival path
//! rests on; after `no_further_dispatch` the seam creates nothing at all and
//! refuses unconditionally.
//!
//! # Boundedness
//!
//! One small entry per tool-invocation request this generation has admitted
//! and not yet forgotten — that is, `O(in-flight tool calls)`, never
//! `O(requests ever raced)`. Every entry has a request-local forget point
//! (its invocation's admission guard together with its outbound
//! participant's decision), so normal completion cleans up its own state
//! without waiting for the connection to close; the end of the outbound seam
//! only clears whatever is still genuinely open. No task is spawned, nothing
//! is retried, and nothing survives the connection generation.

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
        // `Notify::notified()` does not join the waiter list until it is
        // polled or explicitly enabled, and `notify_waiters` stores no
        // permit — so merely *creating* the future before the check leaves a
        // window in which a release reaches no waiter and this call waits
        // forever. Enabling it inside the loop closes that window: the
        // waiter is registered before the flag is read, every iteration.
        loop {
            let notified = self.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.released.load(Ordering::Acquire) {
                return;
            }
            notified.await;
        }
    }
}

/// One tool-invocation request's local lifecycle inside one connection
/// generation.
struct RequestLifecycle {
    /// Cancels every local activity of this request. It exists from the
    /// entry's creation, so a termination that arrives before any local
    /// activity exists still binds every activity that starts afterwards.
    terminate: CancellationToken,
    /// Resolves once no rustX-owned local participant of this request
    /// remains — including one that has not reached the outbound seam yet.
    release: Arc<ReleaseLatch>,
    /// Which local participant currently owns this request, and whether any
    /// can still act.
    phase: RequestPhase,
    /// An MCP invocation still holds this entry. Its admission guard is the
    /// request-local forget point.
    admitted: bool,
    /// The outbound dispatch seam created this entry and the invocation's
    /// admission has not arrived yet. It always does — the executor admits
    /// its request id with no await between the effect frontier and the
    /// admission — so this is only ever open across one interleaving, and it
    /// is what stops a completed exchange from forgetting an entry the
    /// admission would then have to recreate.
    awaiting_admission: bool,
    /// Test-only: whether the outbound participant reached the seam and
    /// **refused** the request, rather than dispatching it.
    #[cfg(test)]
    dispatch_refused: bool,
}

/// Where one request's local ownership is, as a single monotone phase.
///
/// Ownership is a **baton, never two parallel owners**: the outbound
/// participant hands it to the POST the moment the POST registers. Holding
/// both would make a cancellation's local settlement wait for rmcp to
/// resolve an outbound send, which is precisely the dependency the
/// Streamable HTTP contract removes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RequestPhase {
    /// The request is on rmcp's peer outbound queue. Nothing local has
    /// begun, and the outbound participant has not reached the seam — but it
    /// exists, and it is still capable of dispatching this request.
    AwaitingDispatch,
    /// `Transport::send`'s synchronous prologue took dispatch ownership. The
    /// send future holds it until it refuses the request, resolves, is
    /// dropped, or hands the baton to the POST.
    DispatchOwned,
    /// An HTTP request guard owns it: the POST future while it awaits
    /// response headers, and then the SSE response body it produced.
    HttpOwned,
    /// Terminal. Every local participant of this request has been and gone:
    /// the outbound participant refused it, dispatched and released it, or
    /// was proven unable to arrive, and no HTTP owner remains. No local
    /// participant can dispatch this request or hold HTTP activity for it
    /// ever again.
    Released,
}

impl RequestLifecycle {
    fn new(phase: RequestPhase) -> Self {
        Self {
            terminate: CancellationToken::new(),
            release: Arc::new(ReleaseLatch::default()),
            phase,
            admitted: false,
            awaiting_admission: false,
            #[cfg(test)]
            dispatch_refused: false,
        }
    }

    /// Whether this entry has reached its own terminal forget point: every
    /// local participant is terminal and no invocation still holds it.
    const fn forgettable(&self) -> bool {
        matches!(self.phase, RequestPhase::Released) && !self.admitted && !self.awaiting_admission
    }
}

#[derive(Default)]
struct OwnershipState {
    /// One entry per tool-invocation request of this generation that has
    /// been admitted and not yet forgotten.
    requests: HashMap<RequestId, RequestLifecycle>,
    /// rmcp's service loop for this generation has ended, so no further
    /// `Transport::send` can be called for a peer request. From that point
    /// an `AwaitingDispatch` participant is proven unable to arrive, and the
    /// seam creates no lifecycle authority at all.
    dispatch_closed: bool,
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
/// The three cases carry genuinely different settlement evidence, so they
/// are not collapsed: `settled()` is the executor's local settlement bound,
/// and it depends on rustX-owned state only — no remote response, no
/// protocol acknowledgement, and no timer participates in it.
pub(crate) enum LocalRequestTermination {
    /// This transport owns no per-request local state at all. Over stdio an
    /// outbound write owns no resource that outlives it, so there is no
    /// local half of the request to terminate or to prove released.
    NoLocalOwnership,
    /// Every local participant of this request was already terminal before
    /// the termination: the outbound seam had decided and no HTTP owner
    /// remained. Nothing was terminated and nothing is pending. **No record
    /// is created**: this request can never dispatch or register again.
    AlreadyReleased,
    /// A local participant existed and was terminated. The latch resolves
    /// once every one of them is terminal — the outbound participant that
    /// has not reached the seam yet, the send future, the POST future, or
    /// the response body it produced.
    ///
    /// This deliberately covers the pre-dispatch case. A request still on
    /// rmcp's peer outbound queue has a participant that *will* run and is
    /// *capable of dispatching*, so cancelling its token is intent, not
    /// evidence; the evidence is that participant reaching the seam and
    /// refusing, or the generation's outbound seam ending first.
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
        matches!(self, Self::TerminatedPendingRelease(_))
    }

    /// Whether [`Self::settled`] is a real event rather than an
    /// already-settled fact.
    ///
    /// Only a termination that is waiting for a local participant to reach
    /// its terminal state can settle *later* than the caller, so only that
    /// case may be raced against the best-effort remote cancellation send.
    pub(crate) const fn awaits_release(&self) -> bool {
        matches!(self, Self::TerminatedPendingRelease(_))
    }

    /// Awaits the proof that no rustX-owned local participant of this
    /// request remains, and that none can act again.
    pub(crate) async fn settled(&self) {
        match self {
            Self::NoLocalOwnership | Self::AlreadyReleased => (),
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
/// generation closes. It is deliberately *not* sufficient on its own: an
/// entry whose outbound participant has not decided yet outlives the
/// admission, because forgetting it is exactly what would let that
/// participant recreate uncancelled authority.
pub(crate) struct McpRequestAdmission {
    ownership: Arc<McpHttpRequestOwnership>,
    id: RequestId,
}

impl McpRequestAdmission {
    /// Terminates the local ownership of this admitted request.
    ///
    /// Synchronous and unconditional: from the moment it returns, this
    /// request cannot reach the network — the outbound seam refuses every
    /// participant of a cancelled entry, and any POST that somehow still
    /// starts finds the token already cancelled.
    pub(crate) fn terminate(&self) -> LocalRequestTermination {
        self.ownership.terminate(&self.id)
    }
}

impl Drop for McpRequestAdmission {
    fn drop(&mut self) {
        self.ownership.release_admission(&self.id);
    }
}

/// The outbound dispatch participant's hold on one request's lifecycle
/// entry.
///
/// Taken in `Transport::send`'s **synchronous prologue**, so the decision is
/// frozen before the returned send future exists — let alone is polled. The
/// future then carries this guard, and the window in which a POST has not
/// started but is still going to is owned rather than guessed.
pub(crate) struct DispatchOwnership {
    /// `None` over a transport with no per-request lifecycle — stdio, where
    /// an outbound write owns no resource that outlives it.
    ownership: Option<Arc<McpHttpRequestOwnership>>,
    id: Option<RequestId>,
    /// This request's own termination token, captured when ownership was
    /// taken. Reading it needs no lock and is what the send future consults
    /// immediately before it would hand the message to the inner transport.
    terminate: CancellationToken,
}

impl DispatchOwnership {
    /// The ownership of a transport that has no per-request lifecycle.
    pub(crate) fn none() -> Self {
        Self {
            ownership: None,
            id: None,
            terminate: CancellationToken::new(),
        }
    }

    /// Whether the tool invocation this dispatch belongs to has already been
    /// terminated.
    ///
    /// Checked once more after the prologue, immediately before the message
    /// could be handed to the inner transport: a termination that landed in
    /// between must still refuse rather than dispatch.
    pub(crate) fn terminated(&self) -> bool {
        self.terminate.is_cancelled()
    }

    /// This request's own termination token, so a test seam can await the
    /// *applied* termination rather than assume one.
    #[cfg(test)]
    pub(crate) const fn termination(&self) -> &CancellationToken {
        &self.terminate
    }

    /// Consumes this ownership as an explicit refusal: the participant
    /// reached the seam, observed the termination, and will never dispatch.
    ///
    /// This is the fact a pre-dispatch settlement waits for, so it releases
    /// the request's latch exactly as a dropped dispatch does — but it is
    /// recorded distinctly, because "refused" and "dispatched, then
    /// released" are different histories.
    pub(crate) fn refuse(mut self) {
        if let (Some(ownership), Some(id)) = (self.ownership.take(), self.id.take()) {
            ownership.end_dispatch(&id, true);
        }
    }
}

impl Drop for DispatchOwnership {
    fn drop(&mut self) {
        if let (Some(ownership), Some(id)) = (&self.ownership, &self.id) {
            ownership.end_dispatch(id, false);
        }
    }
}

/// What the outbound dispatch seam may do with one request.
pub(crate) enum OutboundDispatch {
    /// The request may go to the transport; the guard owns it until the
    /// outbound send future resolves, refuses, or is dropped.
    Owned(DispatchOwnership),
    /// The request must never be handed to the transport at all: it was
    /// terminated before this participant reached the seam, this generation
    /// no longer dispatches anything, or the request id already has a live
    /// owner. The refusal is recorded on the entry, so a settlement waiting
    /// for this participant is released by it.
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
            // A generation whose outbound seam is over can never dispatch,
            // so an entry created now is born with every participant already
            // terminal rather than waiting for one that cannot arrive.
            let phase = if state.dispatch_closed {
                RequestPhase::Released
            } else {
                RequestPhase::AwaitingDispatch
            };
            let entry = state
                .requests
                .entry(id.clone())
                .or_insert_with(|| RequestLifecycle::new(phase));
            entry.admitted = true;
            entry.awaiting_admission = false;
            Self::settle_entry(&mut state, id);
        }
        McpRequestAdmission {
            ownership: Arc::clone(self),
            id: id.clone(),
        }
    }

    /// Takes dispatch ownership of one outbound tool-invocation request, or
    /// refuses it.
    ///
    /// Called **synchronously inside `Transport::send`**, before the inner
    /// send future is even constructed. That is the linearization point of
    /// the whole lifecycle: from the instant this returns, whether this
    /// request may reach the network is decided, and a termination that
    /// arrives afterwards is answered by the guard rather than by a state
    /// this participant might never read.
    ///
    /// Refusing here is what makes a pre-dispatch settlement honest: the
    /// request never reaches the network, the refusal is recorded on the one
    /// lifecycle entry, and the settlement waiting for this participant is
    /// released by it.
    pub(crate) fn begin_dispatch(self: &Arc<Self>, id: &RequestId) -> OutboundDispatch {
        let mut state = self.state.lock().expect("MCP HTTP ownership lock poisoned");
        if state.dispatch_closed {
            // The generation's outbound seam is over. Nothing it produces
            // may create lifecycle authority for a request again, which is
            // the one path by which a forgotten entry could be resurrected.
            return OutboundDispatch::Refused;
        }
        let entry = state.requests.entry(id.clone()).or_insert_with(|| {
            let mut fresh = RequestLifecycle::new(RequestPhase::AwaitingDispatch);
            fresh.awaiting_admission = true;
            fresh
        });
        let refused = if entry.terminate.is_cancelled() {
            // The invocation already terminated. Consuming the participant
            // here is the evidence its settlement is waiting for.
            if matches!(entry.phase, RequestPhase::AwaitingDispatch) {
                entry.phase = RequestPhase::Released;
                #[cfg(test)]
                {
                    entry.dispatch_refused = true;
                }
            }
            true
        } else if matches!(entry.phase, RequestPhase::AwaitingDispatch) {
            entry.phase = RequestPhase::DispatchOwned;
            false
        } else {
            // Fail closed. One request id is dispatched exactly once per
            // connection generation, so a second outbound send for the same
            // id is either a duplicate or a replay; neither may silently
            // overwrite the ownership state the request already using it
            // holds, and neither may release it.
            true
        };
        if refused {
            Self::settle_entry(&mut state, id);
            return OutboundDispatch::Refused;
        }
        let terminate = state
            .requests
            .get(id)
            .expect("the entry this call just owned")
            .terminate
            .clone();
        drop(state);
        OutboundDispatch::Owned(DispatchOwnership {
            ownership: Some(Arc::clone(self)),
            id: Some(id.clone()),
            terminate,
        })
    }

    /// Registers one request's in-flight HTTP activity and returns the guard
    /// that owns it.
    ///
    /// # The baton is a precondition, not a formality
    ///
    /// A tracked `tools/call` POST exists only because the send future this
    /// seam gave dispatch ownership to handed the message to rmcp's worker,
    /// and that future holds its ownership until the worker answers. A
    /// tracked registration therefore **requires `DispatchOwned`**, and
    /// every other phase is refused with a pre-cancelled, untracked guard:
    ///
    /// - `Released` — the request is terminal. Re-entering HTTP ownership
    ///   from there would resurrect a request whose release proof has
    ///   already been published and whose settlement may already have been
    ///   reported;
    /// - `HttpOwned` — a live HTTP ownership already exists and is not
    ///   overwritten;
    /// - `AwaitingDispatch` — no participant has taken the baton, so no POST
    ///   of this request can exist yet.
    ///
    /// A request with no lifecycle entry — every request rustX sends that is
    /// not a tool invocation — owns no state a settlement could terminate
    /// and is registered as untracked.
    ///
    /// The guard's token is already cancelled when the request was
    /// terminated after its dispatch was owned, so the POST is
    /// pre-terminated instead of reaching the network.
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
        if !matches!(entry.phase, RequestPhase::DispatchOwned) {
            // Fail closed: the registration is given a pre-cancelled,
            // untracked guard, so it cannot reach the network and cannot
            // release ownership that is not its own.
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
        entry.phase = RequestPhase::HttpOwned;
        let terminate = entry.terminate.clone();
        drop(state);
        RequestOwnershipGuard {
            ownership: Some(Arc::clone(self)),
            id,
            terminate,
        }
    }

    /// Terminates the local ownership of one request, reading its explicit
    /// phase rather than inferring one from an absent record.
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
        match entry.phase {
            RequestPhase::Released => LocalRequestTermination::AlreadyReleased,
            RequestPhase::AwaitingDispatch
            | RequestPhase::DispatchOwned
            | RequestPhase::HttpOwned => {
                LocalRequestTermination::TerminatedPendingRelease(Arc::clone(&entry.release))
            }
        }
    }

    /// Terminates every request this generation still owns locally.
    ///
    /// Connection close calls this before it awaits rmcp's service shutdown,
    /// so drain owns the per-request control primitives introduced here
    /// rather than inheriting them. It decides nothing: an outbound
    /// participant that has not reached the seam yet is still capable of
    /// dispatching at this instant, and only [`Self::no_further_dispatch`]
    /// may claim otherwise.
    pub(crate) fn terminate_all(&self) {
        let mut state = self.state.lock().expect("MCP HTTP ownership lock poisoned");
        for entry in state.requests.values_mut() {
            entry.terminate.cancel();
        }
    }

    /// Declares that this generation's outbound seam is over: no further
    /// `Transport::send` can be called for a peer request.
    ///
    /// rmcp calls `Transport::close` from its service loop **after** that
    /// loop has broken out of its event select, and dropping the transport
    /// is the backstop for every path that never reaches close, so this is
    /// the exact instant an `AwaitingDispatch` participant becomes provably
    /// unable to arrive. Publishing that fact is what keeps a pre-dispatch
    /// settlement bounded when the connection dies underneath it, and it is
    /// the only place allowed to reach `Released` without a participant's
    /// own decision.
    pub(crate) fn no_further_dispatch(&self) {
        let mut state = self.state.lock().expect("MCP HTTP ownership lock poisoned");
        state.dispatch_closed = true;
        for entry in state.requests.values_mut() {
            entry.terminate.cancel();
            if matches!(entry.phase, RequestPhase::AwaitingDispatch) {
                entry.phase = RequestPhase::Released;
                entry.release.release();
            }
            // No dispatch of this generation can arrive, so an entry kept
            // open only to bind an admission that never came has nothing
            // left to protect.
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
        if matches!(entry.phase, RequestPhase::HttpOwned) {
            entry.phase = RequestPhase::Released;
        }
        Self::settle_entry(&mut state, id);
    }

    /// Ends one request's outbound dispatch participation.
    ///
    /// `refused` records *why*: the participant observed the termination and
    /// never handed the message on, rather than having dispatched it and
    /// then released. Either way the participant is terminal, which is what
    /// the release proof is about.
    ///
    /// A no-op once the POST has taken the baton: from that point the HTTP
    /// guard is what owns the request, and the outbound send resolving says
    /// nothing about it.
    fn end_dispatch(&self, id: &RequestId, refused: bool) {
        let mut state = self.state.lock().expect("MCP HTTP ownership lock poisoned");
        let Some(entry) = state.requests.get_mut(id) else {
            return;
        };
        if matches!(entry.phase, RequestPhase::DispatchOwned) {
            entry.phase = RequestPhase::Released;
            #[cfg(test)]
            {
                entry.dispatch_refused = refused;
            }
        }
        let _ = refused;
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

    /// Fires the release proof once every local participant is terminal, and
    /// forgets the entry once nothing holds it at all.
    ///
    /// The latch is released *after* the caller has already dropped the
    /// object that owned the request — the POST future, the response body,
    /// or the outbound send future — so awaiting it is ownership evidence
    /// rather than a restatement of "we stopped waiting".
    fn settle_entry(state: &mut OwnershipState, id: &RequestId) {
        let Some(entry) = state.requests.get_mut(id) else {
            return;
        };
        if matches!(entry.phase, RequestPhase::Released) {
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

    /// The explicit lifecycle phase of one request, when it is still known.
    #[cfg(test)]
    fn phase_of(&self, id: &RequestId) -> Option<RequestPhase> {
        self.state
            .lock()
            .expect("MCP HTTP ownership lock poisoned")
            .requests
            .get(id)
            .map(|entry| entry.phase)
    }

    /// Whether one request's outbound participant reached the seam and
    /// refused, when the entry is still known.
    #[cfg(test)]
    fn dispatch_refused(&self, id: &RequestId) -> Option<bool> {
        self.state
            .lock()
            .expect("MCP HTTP ownership lock poisoned")
            .requests
            .get(id)
            .map(|entry| entry.dispatch_refused)
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
/// timer: every transition is driven explicitly, so "the outbound
/// participant has not arrived yet", "the POST has not registered yet" and
/// "the POST already finished" are three different proven states rather than
/// three readings of the same missing record.
#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use rmcp::model::RequestId;

    use super::{LocalRequestTermination, McpHttpRequestOwnership, OutboundDispatch, RequestPhase};

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

    /// Whether a future is still pending after every already-runnable task
    /// of this single-threaded test has had a chance to run.
    ///
    /// This is a scheduling fact, not a timing one: `yield_now` hands the
    /// runtime every task that is ready, so a proof that stays pending
    /// across it is pending because nothing has released it.
    async fn still_pending(future: impl std::future::Future<Output = ()>) -> bool {
        tokio::pin!(future);
        for _ in 0..64 {
            if futures_util::poll!(future.as_mut()).is_ready() {
                return false;
            }
            tokio::task::yield_now().await;
        }
        true
    }

    /// The release proof is a proof: it must not resolve while any local
    /// participant of the request — the outbound participant that has not
    /// reached the seam, the send future, the POST future, or the response
    /// body that future produced — can still act.
    #[tokio::test]
    async fn the_release_proof_resolves_only_after_every_local_owner_is_dropped() {
        let ownership = Arc::new(McpHttpRequestOwnership::default());
        let admission = ownership.admit(&id(1));
        let dispatching = dispatch(&ownership, 1);
        // While only the outbound send owns the request, that is the owner a
        // termination has to wait for.
        let pre_registration = admission.terminate();
        assert!(pre_registration.awaits_release());
        assert!(
            still_pending(pre_registration.settled()).await,
            "the proof does not resolve while the outbound dispatch owns the request"
        );
        let guard = ownership.register(id(1));
        assert!(
            guard.terminate.is_cancelled(),
            "the POST inherits the termination and is pre-terminated"
        );
        // The baton passed to the POST, so the outbound send future
        // resolving is no longer what settlement waits for.
        drop(dispatching);
        assert!(
            still_pending(pre_registration.settled()).await,
            "the proof does not resolve while the POST owns the request"
        );
        drop(guard);
        tokio::time::timeout(
            std::time::Duration::from_secs(30),
            pre_registration.settled(),
        )
        .await
        .expect("anti-hang guard: dropping the owning guard releases the proof");
        drop(admission);
        assert_eq!(ownership.outstanding_requests(), 0);
    }

    /// **Cancellation before the outbound seam consumed the request.** The
    /// request is admitted, rmcp has not called `Transport::send` for it yet,
    /// and the cancellation wins.
    ///
    /// The settlement is *not* immediate, and that is the whole finding: an
    /// outbound participant exists on rmcp's peer queue and is still capable
    /// of dispatching. Settlement completes exactly when that participant
    /// arrives and refuses — never merely because a token was set.
    #[tokio::test]
    async fn cancellation_before_dispatch_settles_only_when_the_seam_refuses() {
        let ownership = Arc::new(McpHttpRequestOwnership::default());
        let admission = ownership.admit(&id(7));
        assert_eq!(
            ownership.phase_of(&id(7)),
            Some(RequestPhase::AwaitingDispatch),
            "the outbound participant exists and has not reached the seam"
        );
        let termination = admission.terminate();
        assert!(termination.terminated_local_request());
        assert!(
            termination.awaits_release(),
            "an outbound participant that can still dispatch is a pending local owner"
        );
        assert!(
            still_pending(termination.settled()).await,
            "settlement cannot complete while an outbound participant may still dispatch"
        );
        // The outbound seam is only reached now — and refuses, so the
        // request never leaves rustX.
        assert!(
            matches!(ownership.begin_dispatch(&id(7)), OutboundDispatch::Refused),
            "a terminated request is never handed to the transport"
        );
        assert_eq!(
            ownership.dispatch_refused(&id(7)),
            Some(true),
            "the refusal is recorded on the one lifecycle entry"
        );
        tokio::time::timeout(std::time::Duration::from_secs(30), termination.settled())
            .await
            .expect("anti-hang guard: the refusal releases the settlement");
        // Any POST that somehow still started owns no baton and can never
        // reach the network.
        assert!(
            ownership.register(id(7)).terminate.is_cancelled(),
            "a request with no dispatch ownership never registers an HTTP owner"
        );
        drop(admission);
        assert_eq!(
            ownership.outstanding_requests(),
            0,
            "the invocation's own forget point clears the entry"
        );
    }

    /// **The connection dies before the outbound participant arrives.** The
    /// participant can never reach the seam, so the generation says so
    /// explicitly rather than leaving the settlement waiting forever.
    #[tokio::test]
    async fn an_outbound_participant_that_can_never_arrive_settles_the_termination() {
        let ownership = Arc::new(McpHttpRequestOwnership::default());
        let admission = ownership.admit(&id(21));
        let termination = admission.terminate();
        assert!(
            still_pending(termination.settled()).await,
            "the participant has not arrived and has not been proven unable to"
        );
        // rmcp's service loop ended: `Transport::close` (or the transport's
        // own drop) publishes that no further send can be called.
        ownership.no_further_dispatch();
        tokio::time::timeout(std::time::Duration::from_secs(30), termination.settled())
            .await
            .expect("anti-hang guard: a participant proven unable to arrive settles it");
        assert!(
            matches!(ownership.begin_dispatch(&id(21)), OutboundDispatch::Refused),
            "the closed seam refuses unconditionally"
        );
        drop(admission);
        assert_eq!(ownership.outstanding_requests(), 0);
    }

    /// **A stale outbound continuation can never recreate lifecycle
    /// authority.** This is the review finding, pinned at the state machine
    /// with no transport and no timer.
    ///
    /// The old shape settled a pre-dispatch cancellation immediately, let
    /// the admission drop forget the entry, and then let the outbound
    /// participant's `entry(id).or_insert_with(..)` mint a **fresh,
    /// uncancelled** entry — dispatching a `tools/call` after its canonical
    /// terminal result already existed.
    #[tokio::test]
    async fn a_stale_outbound_continuation_never_resurrects_a_terminal_request() {
        let ownership = Arc::new(McpHttpRequestOwnership::default());
        let admission = ownership.admit(&id(31));
        let termination = admission.terminate();
        // The invocation is done with its entry: it settles, reports, and
        // drops its admission. Under the old shape this was the point at
        // which the entry disappeared.
        drop(admission);
        assert_eq!(
            ownership.phase_of(&id(31)),
            Some(RequestPhase::AwaitingDispatch),
            "the entry outlives the admission precisely because a participant may still act"
        );
        // The stale participant finally runs.
        assert!(
            matches!(ownership.begin_dispatch(&id(31)), OutboundDispatch::Refused),
            "the participant attaches to the lifecycle entry it belongs to and fails closed"
        );
        assert_eq!(
            ownership.phase_of(&id(31)),
            None,
            "the refusal is the entry's last transition, so it is forgotten by it"
        );
        tokio::time::timeout(std::time::Duration::from_secs(30), termination.settled())
            .await
            .expect("anti-hang guard: the refusal releases the settlement");
        assert_eq!(
            ownership.outstanding_requests(),
            0,
            "the registry reaches zero once every participant is terminal"
        );
        // And a participant arriving after the entry is gone creates nothing
        // executable: one request id gets one monotone lifecycle.
        ownership.no_further_dispatch();
        assert!(
            matches!(ownership.begin_dispatch(&id(31)), OutboundDispatch::Refused),
            "a terminal request id never transitions back to executable"
        );
        assert_eq!(ownership.outstanding_requests(), 0);
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
        assert_eq!(ownership.phase_of(&id(2)), Some(RequestPhase::HttpOwned));
        let termination = admission.terminate();
        assert!(
            termination.awaits_release(),
            "a live local owner must be proven released before settlement"
        );
        // The outbound send future resolving is deliberately not part of the
        // proof: settlement must never depend on rmcp servicing a send.
        drop(dispatching);
        assert!(
            still_pending(termination.settled()).await,
            "the POST still owns this request"
        );
        drop(guard);
        tokio::time::timeout(std::time::Duration::from_secs(30), termination.settled())
            .await
            .expect("anti-hang guard: the release proof resolves");
        assert_eq!(ownership.phase_of(&id(2)), Some(RequestPhase::Released));
    }

    /// **The response released before `handle.rx` delivery.** This is the
    /// race the previous shape could not see.
    ///
    /// Every local participant of the request is terminal — the POST
    /// finished, its body is gone, and the outbound send already handed the
    /// baton on — but rmcp has not delivered the correlated response into
    /// the executor's channel yet, so a deadline or cancellation can still
    /// land here. Under the old shape the request id was simply absent from
    /// the live map, which was read as "the POST has not started yet", and a
    /// pre-termination record was created for a request that could never
    /// register again and then survived until the connection generation
    /// closed.
    #[tokio::test]
    async fn cancellation_after_release_creates_no_record_and_leaks_nothing() {
        let ownership = Arc::new(McpHttpRequestOwnership::default());
        let admission = ownership.admit(&id(3));
        let dispatching = dispatch(&ownership, 3);
        let guard = ownership.register(id(3));
        // The HTTP exchange completes normally: body consumed, POST done.
        // The outbound send future is still pending — rmcp has not resolved
        // it yet — and that deliberately does not hold the request open,
        // because the baton left it when the POST registered.
        drop(guard);
        assert_eq!(
            ownership.phase_of(&id(3)),
            Some(RequestPhase::Released),
            "the completed request is known to be released, not inferred to be unstarted"
        );
        // The correlated response has not reached the executor yet; the
        // deadline fires here.
        let termination = admission.terminate();
        assert!(
            matches!(termination, LocalRequestTermination::AlreadyReleased),
            "every local participant is already terminal, so nothing was terminated"
        );
        assert!(
            !termination.terminated_local_request(),
            "nothing was terminated, so a transport failure is still the transport's own fact"
        );
        termination.settled().await;
        // No future dispatch and no future registration can occur, and no
        // record was created to stop one.
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

    /// **`Released -> HttpOwned` is impossible.** A tracked POST registration
    /// requires the dispatch baton, so a released request cannot acquire a
    /// second HTTP owner, and its release proof stays terminal.
    #[tokio::test]
    async fn a_released_request_never_takes_http_ownership_again() {
        let ownership = Arc::new(McpHttpRequestOwnership::default());
        let admission = ownership.admit(&id(41));
        let dispatching = dispatch(&ownership, 41);
        let live = ownership.register(id(41));
        assert_eq!(ownership.phase_of(&id(41)), Some(RequestPhase::HttpOwned));
        drop(live);
        drop(dispatching);
        assert_eq!(ownership.phase_of(&id(41)), Some(RequestPhase::Released));
        // The release proof has already been published for this request.
        let release_proof = admission.terminate();
        assert!(matches!(
            release_proof,
            LocalRequestTermination::AlreadyReleased
        ));

        // A stale or duplicate POST registration arrives.
        let stale = ownership.register(id(41));
        assert!(
            stale.terminate.is_cancelled(),
            "a registration with no dispatch baton is pre-cancelled and never reaches the network"
        );
        assert_eq!(
            ownership.phase_of(&id(41)),
            Some(RequestPhase::Released),
            "the released request stays released: no second HTTP owner appears"
        );
        // Dropping the refused guard releases nothing that belongs to the
        // request, and the original release proof stays terminal.
        drop(stale);
        assert_eq!(ownership.phase_of(&id(41)), Some(RequestPhase::Released));
        release_proof.settled().await;
        // A duplicate dispatch after release fails closed the same way.
        assert!(
            matches!(ownership.begin_dispatch(&id(41)), OutboundDispatch::Refused),
            "one request id gets one monotone lifecycle"
        );
        assert_eq!(ownership.phase_of(&id(41)), Some(RequestPhase::Released));
        drop(admission);
        assert_eq!(ownership.outstanding_requests(), 0);
    }

    /// A POST cannot register before the outbound participant has taken the
    /// baton: no local activity of a request can exist before its dispatch
    /// is owned.
    #[tokio::test]
    async fn a_post_without_the_dispatch_baton_is_refused() {
        let ownership = Arc::new(McpHttpRequestOwnership::default());
        let admission = ownership.admit(&id(43));
        let premature = ownership.register(id(43));
        assert!(
            premature.terminate.is_cancelled(),
            "an AwaitingDispatch request has no POST to register"
        );
        assert_eq!(
            ownership.phase_of(&id(43)),
            Some(RequestPhase::AwaitingDispatch),
            "the refused registration changed nothing"
        );
        drop(premature);
        let dispatching = dispatch(&ownership, 43);
        let live = ownership.register(id(43));
        assert!(!live.terminate.is_cancelled());
        drop(live);
        drop(dispatching);
        drop(admission);
        assert_eq!(ownership.outstanding_requests(), 0);
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
        let terminations: Vec<_> = admissions
            .iter()
            .map(McpRequestAdmissionExt::terminate_admission)
            .collect();
        assert!(
            terminations
                .iter()
                .all(LocalRequestTermination::awaits_release),
            "each one waits for its own outbound participant"
        );
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
        for termination in &terminations {
            tokio::time::timeout(std::time::Duration::from_secs(30), termination.settled())
                .await
                .expect("anti-hang guard: every refusal releases its own settlement");
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
        assert_eq!(
            ownership.phase_of(&id(5)),
            Some(RequestPhase::DispatchOwned),
            "the refused duplicate did not disturb the live dispatch ownership"
        );
        let live = ownership.register(id(5));
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
        assert_eq!(ownership.phase_of(&id(5)), Some(RequestPhase::HttpOwned));
        drop(live);
        assert_eq!(ownership.phase_of(&id(5)), Some(RequestPhase::Released));
        drop(dispatching);
        drop(admission);
        assert_eq!(ownership.outstanding_requests(), 0);
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

    /// The generation's outbound seam ending terminates every request it
    /// still owns, so drain inherits no live per-request control primitive —
    /// and it clears the entries that only existed to bind a dispatch that
    /// can no longer happen.
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
        // Runtime close cancels every request; it decides nothing about
        // participants that have not arrived.
        ownership.terminate_all();
        assert!(guards.iter().all(|guard| guard.terminate.is_cancelled()));
        // The transport's own close then publishes that no dispatch can
        // arrive again.
        ownership.no_further_dispatch();
        drop(guards);
        drop(dispatches);
        drop(admissions);
        assert_eq!(
            ownership.outstanding_requests(),
            0,
            "close leaves no request lifecycle state alive"
        );
    }

    /// A borrow-free way to call [`super::McpRequestAdmission::terminate`]
    /// through a shared reference in an iterator chain.
    trait McpRequestAdmissionExt {
        fn terminate_admission(&self) -> LocalRequestTermination;
    }

    impl McpRequestAdmissionExt for super::McpRequestAdmission {
        fn terminate_admission(&self) -> LocalRequestTermination {
            self.terminate()
        }
    }
}
