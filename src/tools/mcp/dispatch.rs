//! The rustX-owned outbound dispatch seam (Issue #205).
//!
//! # Why a transport seam, and why exactly here
//!
//! Two contracts need a fact rmcp's public request API cannot give a caller,
//! and both need it at the same instant:
//!
//! - **progress liveness.** rmcp mints a request's `ProgressToken` *inside*
//!   `Peer::send_cancellable_request`, so the dispatching call learns it only
//!   after the request was enqueued. Anything that arrives for that token
//!   before the call subscribes has no owner, and a router that bounds
//!   unowned tokens by capacity can evict a legitimate live request's only
//!   liveness evidence — nothing bounds how many admitted requests are inside
//!   that window at once;
//! - **local request ownership.** Over Streamable HTTP a settlement has to
//!   terminate *this request's* local half and prove it released. Inferring
//!   "the POST has not started yet" from a request id being absent from a
//!   live map cannot distinguish that from "the POST already finished".
//!
//! Both are answered by owning the request at the outbound transport. rmcp's
//! [`Transport::send`] is called with the fully-formed
//! [`ClientJsonRpcMessage`] — request id and `_meta.progressToken` already
//! set — and its **synchronous prologue runs before the message is handed to
//! the transport at all**. That prologue is therefore a linearization point
//! with a causal, not merely temporal, guarantee:
//!
//! ```text
//! Transport::send prologue        registers (RequestId, ProgressToken)
//!     |                           and takes dispatch ownership
//!     v
//! bytes on the wire
//!     v
//! the server receives the request
//!     v
//! the server may emit progress for that token
//!     v
//! the router delivers it                    <-- always after registration
//! ```
//!
//! There is consequently **no unowned request-token window**: a progress
//! notification for a rustX request can never reach the router before that
//! request's token is known and live, so a known token never has to compete
//! with peer-controlled traffic for capacity.
//!
//! # Why ownership is taken in the prologue and not in the future
//!
//! rmcp's service loop *calls* `Transport::send` in its own event-handling
//! body and only then spawns the returned future onto a `JoinSet`. Those are
//! two different instants:
//!
//! ```text
//! A  the request is accepted onto the peer's outbound mpsc
//! B  the service loop dequeues it
//! C  Transport::send(..) is called          <-- the synchronous prologue
//! D  the returned future is first polled    <-- can be much later
//! E  rmcp's worker transport receives it
//! F  the Streamable HTTP POST begins
//! G  the remote effect occurs
//! ```
//!
//! Taking dispatch ownership inside the returned future — at `D` — leaves
//! `C..D` owned by nobody while a fully-formed request sits inside a live
//! future that will dispatch it. A cancellation landing in that window used
//! to see "nothing local began", settle, drop the invocation's admission,
//! and let the entry be forgotten; the future then woke, found no entry, and
//! **created a fresh uncancelled one**, dispatching a `tools/call` after its
//! canonical terminal result already existed.
//!
//! So the decision is frozen at `C`, before the future exists:
//! [`crate::tools::mcp::streamable_http::McpHttpRequestOwnership::begin_dispatch`]
//! either grants ownership or refuses, and a refused request never even has
//! an inner send future constructed for it — which matters, because rmcp's
//! `WorkerTransport::send` registers a request cancellation entry in its own
//! synchronous prologue. `A..C` is not a gap either: it is the lifecycle's
//! explicit `AwaitingDispatch` phase, and a settlement there waits for this
//! seam to arrive and refuse.
//!
//! One more check happens after the prologue and immediately before the
//! inner send could be polled, because a termination can land in `C..D`
//! too. If it did, the inner send future is dropped **unpolled** and the
//! ownership is consumed as an explicit refusal, which is the fact the
//! waiting settlement is released by.
//!
//! # The inbound half: a response is not a caller's disappearance
//!
//! The same seam observes inbound messages, and it correlates exactly one
//! fact there: *this request will receive no further correlated answer*.
//!
//! ```text
//! ServerJsonRpcMessage::Response / ::Error   -> the remote request is over
//! OutboundDispatch::Refused                  -> it never started, and never will
//! ```
//!
//! Neither says anything about the **dispatching call**, which may still be
//! between its effect frontier and its progress subscription — rmcp mints
//! the progress token inside the request, so that window is unavoidable and
//! a fast server can answer inside it. Reading a correlated response as
//! proof that no caller remains deleted the very evidence
//! [`crate::tools::mcp::McpProgressRouter`] exists to keep. So this seam
//! only ever completes the *remote* dimension of a request's progress
//! ownership; the caller's own `McpProgressLease` completes the other, and
//! the router forgets the request when both are terminal.
//!
//! # What this seam is not
//!
//! It is an ownership seam, not a policy layer and not a second protocol
//! implementation. It reads two fields, records them, and delegates. It
//! decodes nothing, correlates nothing, retries nothing, and spawns nothing.
//! The one decision it makes is refusing to hand an already-terminated tool
//! invocation to the transport, which is what lets a cancellation that won
//! before dispatch report an honest settlement instead of a hope.

use std::future::Future;
use std::sync::Arc;

use rmcp::model::{
    ClientJsonRpcMessage, ClientRequest, GetMeta as _, ProgressToken, RequestId,
    ServerJsonRpcMessage,
};
use rmcp::service::RoleClient;
use rmcp::transport::Transport;

use super::McpProgressRouter;
use super::streamable_http::{DispatchOwnership, McpHttpRequestOwnership, OutboundDispatch};

/// The transport error of a request rustX refused to dispatch.
///
/// It is deliberately reported as an ordinary transport-send failure: rmcp
/// resolves the request's local responder from it, so the dispatching call
/// observes its own termination through the same path any other send failure
/// takes. The MCP executor knows it terminated the request and therefore does
/// not read this as evidence about the connection generation's health.
#[derive(Debug)]
pub(crate) enum McpTransportError<E> {
    /// The underlying transport's own failure.
    Transport(E),
    /// The request's tool call was already settled, so the request was never
    /// handed to the transport and never reached the network.
    LocallyTerminated,
}

impl<E: std::fmt::Display> std::fmt::Display for McpTransportError<E> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Transport(error) => error.fmt(formatter),
            Self::LocallyTerminated => formatter.write_str(
                "the MCP request was terminated by rustX before dispatch because its tool \
                 call was cancelled, so it never reached the transport",
            ),
        }
    }
}

impl<E: std::error::Error + 'static> std::error::Error for McpTransportError<E> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Transport(error) => Some(error),
            Self::LocallyTerminated => None,
        }
    }
}

/// The per-connection-generation state the seam owns on behalf of requests.
pub(crate) struct McpDispatchSeam {
    progress: Arc<McpProgressRouter>,
    /// Present only for Streamable HTTP: stdio owns no per-request local
    /// state, so there is no lifecycle for this seam to open there.
    ownership: Option<Arc<McpHttpRequestOwnership>>,
}

impl McpDispatchSeam {
    pub(crate) const fn new(
        progress: Arc<McpProgressRouter>,
        ownership: Option<Arc<McpHttpRequestOwnership>>,
    ) -> Self {
        Self {
            progress,
            ownership,
        }
    }
}

/// One outbound tool invocation, as the seam sees it.
///
/// Only `tools/call` requests are tracked. They are exactly the requests an
/// MCP settlement can ever need to terminate and exactly the requests whose
/// progress is a live `ToolCall`'s idle-liveness evidence, so tracking
/// anything else would create lifecycle state with no invocation to forget
/// it.
struct ToolInvocation {
    id: RequestId,
    token: Option<ProgressToken>,
    #[cfg_attr(not(test), allow(dead_code))]
    tool: String,
}

fn tool_invocation(message: &ClientJsonRpcMessage) -> Option<ToolInvocation> {
    let ClientJsonRpcMessage::Request(request) = message else {
        return None;
    };
    let ClientRequest::CallToolRequest(call) = &request.request else {
        return None;
    };
    Some(ToolInvocation {
        id: request.id.clone(),
        token: request.request.get_meta().get_progress_token(),
        tool: call.params.name.to_string(),
    })
}

/// The JSON-RPC request id one inbound message answers, when it answers one.
fn answered_request(message: &ServerJsonRpcMessage) -> Option<&RequestId> {
    match message {
        ServerJsonRpcMessage::Response(response) => Some(&response.id),
        ServerJsonRpcMessage::Error(error) => error.id.as_ref(),
        _ => None,
    }
}

impl McpDispatchSeam {
    /// The outbound linearization point.
    ///
    /// Registers the request's progress token as known and live before the
    /// message can be handed to the transport, which is what removes the
    /// unowned-token window entirely. It runs in `Transport::send`'s
    /// synchronous prologue, so the registration *happens-before* the bytes
    /// can leave rustX.
    fn admit_progress(&self, message: &ClientJsonRpcMessage) -> Option<ToolInvocation> {
        let invocation = tool_invocation(message)?;
        if let Some(token) = &invocation.token {
            self.progress.admit(&invocation.id, token);
        }
        Some(invocation)
    }

    /// Takes dispatch ownership of one tool invocation inside
    /// `Transport::send`'s synchronous prologue.
    ///
    /// Deliberately as early as the seam can see the request: every
    /// cancellation that lands before this point is refused here, and every
    /// cancellation that lands after it finds a lifecycle entry with an
    /// owner, so no interval of the request is unowned.
    fn begin_dispatch(&self, id: &RequestId) -> OutboundDispatch {
        self.ownership.as_ref().map_or_else(
            || OutboundDispatch::Owned(DispatchOwnership::none()),
            |ownership| ownership.begin_dispatch(id),
        )
    }

    /// Declares this generation's outbound seam over, so a request still
    /// waiting for it can stop waiting.
    fn no_further_dispatch(&self) {
        if let Some(ownership) = &self.ownership {
            ownership.no_further_dispatch();
        }
    }

    /// The inbound terminal-correlation point.
    ///
    /// A correlated answer — a `CallToolResult` or a JSON-RPC error for this
    /// request id — ends the request's **remote** execution. It is
    /// deliberately not read as evidence about the dispatching call: that
    /// call may still be between its effect frontier and its subscription,
    /// and deleting the progress it has not claimed yet would discard
    /// genuine liveness evidence it is still entitled to. The router forgets
    /// the request only once the caller has relinquished it too.
    fn observe_inbound(&self, message: &ServerJsonRpcMessage) {
        if let Some(id) = answered_request(message) {
            self.progress.mark_remote_terminal(id);
        }
    }

    /// The outbound terminal-correlation point.
    ///
    /// A request rustX refused to dispatch never reaches the network, so no
    /// correlated response can ever arrive for it. That is the same remote
    /// terminality an answer carries, and recording it is what gives a
    /// refused request the request-local forget point an inbound answer
    /// would otherwise have provided.
    fn no_dispatch(&self, id: &RequestId) {
        self.progress.mark_remote_terminal(id);
    }
}

/// The transport rustX hands to rmcp: the unmodified inner transport, plus
/// the outbound ownership seam.
pub(crate) struct ObservingTransport<T> {
    inner: T,
    seam: Arc<McpDispatchSeam>,
}

impl<T> ObservingTransport<T> {
    pub(crate) const fn new(inner: T, seam: Arc<McpDispatchSeam>) -> Self {
        Self { inner, seam }
    }
}

/// Awaits the inner transport's send, when one was constructed.
///
/// The `None` arm is unreachable by construction — the prologue builds an
/// inner send for every message it does not refuse, and every refusal
/// returns before this point — and it is written as a locally terminated
/// request rather than a panic so a future edit cannot turn an ownership
/// mistake into a runtime abort.
async fn deliver<F, E>(inner: Option<F>) -> Result<(), McpTransportError<E>>
where
    F: Future<Output = Result<(), E>> + Send,
{
    match inner {
        Some(send) => send.await.map_err(McpTransportError::Transport),
        None => Err(McpTransportError::LocallyTerminated),
    }
}

impl<T> Transport<RoleClient> for ObservingTransport<T>
where
    T: Transport<RoleClient> + Send,
{
    type Error = McpTransportError<T::Error>;

    fn send(
        &mut self,
        item: ClientJsonRpcMessage,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send + 'static {
        // ---- the synchronous prologue: everything below is decided before
        // ---- this function returns, and therefore before rmcp can poll,
        // ---- delay, or drop anything belonging to this request.
        //
        // The progress-token linearization point the whole module exists for.
        let invocation = self.seam.admit_progress(&item);
        // The dispatch linearization point. Frozen here rather than in the
        // returned future, so no interval of this request is unowned.
        let dispatch = invocation
            .as_ref()
            .map(|invocation| self.seam.begin_dispatch(&invocation.id));
        // A refused request never even has an inner send constructed for it:
        // rmcp's worker transport registers the request in its own
        // synchronous prologue, so constructing one would leave transport
        // state behind for a request that must not exist below rustX.
        let inner = match &dispatch {
            Some(OutboundDispatch::Refused) => None,
            Some(OutboundDispatch::Owned(_)) | None => Some(self.inner.send(item)),
        };
        #[cfg(test)]
        let probe = invocation
            .as_ref()
            .map(|invocation| (invocation.tool.clone(), invocation.id.clone()));
        // The refusal arms below report this request's remote terminality,
        // so the seam that admitted its progress token is also the seam that
        // releases it when the request never reaches the network.
        let invocation_id = invocation.map(|invocation| invocation.id);
        let seam = Arc::clone(&self.seam);
        async move {
            let guard = match dispatch {
                // Not a tool invocation: no settlement can ever terminate it,
                // so it owns nothing and passes straight through.
                None => return deliver(inner).await,
                // Already terminated when the prologue ran, or a duplicate
                // send for a live request id. Nothing of this request exists
                // below rustX, and the refusal was recorded on its lifecycle
                // entry, which is what releases the settlement waiting for
                // this participant.
                Some(OutboundDispatch::Refused) => {
                    if let Some(id) = &invocation_id {
                        seam.no_dispatch(id);
                    }
                    #[cfg(test)]
                    if let Some((tool, id)) = &probe {
                        crate::tools::mcp::test_sync::note_outbound_dispatch(tool, id, true);
                    }
                    return Err(McpTransportError::LocallyTerminated);
                }
                Some(OutboundDispatch::Owned(guard)) => guard,
            };
            // Test-only: holds one owned tool invocation between the
            // prologue and the first poll of the inner send — the exact
            // window in which the request has an outbound participant and
            // has not reached the transport. No production path installs a
            // pause.
            #[cfg(test)]
            if let Some((tool, id)) = &probe {
                crate::tools::mcp::test_sync::park_before_outbound_dispatch(
                    tool,
                    id,
                    guard.termination(),
                )
                .await;
            }
            // The last decision before any remote effect. A termination that
            // landed after the prologue must still refuse: dropping the
            // inner send future *without polling it* is what keeps the
            // request off the network entirely.
            if guard.terminated() {
                drop(inner);
                if let Some(id) = &invocation_id {
                    seam.no_dispatch(id);
                }
                // Recorded *before* the ownership is consumed. Consuming it
                // publishes this request's release proof, which can settle a
                // waiting invocation immediately — so a decision recorded
                // afterwards would be a fact that lags the very settlement
                // it is evidence for, and a regression waiting on it would
                // race the runtime rather than order it.
                #[cfg(test)]
                if let Some((tool, id)) = &probe {
                    crate::tools::mcp::test_sync::note_outbound_dispatch(tool, id, true);
                }
                guard.refuse();
                return Err(McpTransportError::LocallyTerminated);
            }
            #[cfg(test)]
            if let Some((tool, id)) = &probe {
                crate::tools::mcp::test_sync::note_outbound_dispatch(tool, id, false);
            }
            // Dispatch ownership lives exactly as long as this send does,
            // unless the POST takes the baton first.
            let _guard = guard;
            deliver(inner).await
        }
    }

    async fn receive(&mut self) -> Option<ServerJsonRpcMessage> {
        let message = self.inner.receive().await;
        if let Some(message) = &message {
            self.seam.observe_inbound(message);
        }
        message
    }

    async fn close(&mut self) -> Result<(), Self::Error> {
        // rmcp calls this from its service loop *after* that loop has broken
        // out of its event select, so no further peer request can reach
        // `send`. Publishing that fact here is what bounds a settlement
        // still waiting for an outbound participant that will now never
        // arrive.
        self.seam.no_further_dispatch();
        self.inner
            .close()
            .await
            .map_err(McpTransportError::Transport)
    }
}

/// The backstop for every path that ends the service loop without reaching
/// [`Transport::close`] — an aborted serve task, or a runtime shutdown that
/// simply drops it. The transport is the service loop's own local, so its
/// drop is the last instant at which a new outbound dispatch could have
/// existed.
impl<T> Drop for ObservingTransport<T> {
    fn drop(&mut self) {
        self.seam.no_further_dispatch();
    }
}

/// Deterministic regressions for the seam's correlation rules (Issue #205).
#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use rmcp::model::{
        CallToolRequest, CallToolRequestParams, ClientJsonRpcMessage, ClientRequest, ErrorData,
        GetMeta as _, JsonRpcMessage, NumberOrString, ProgressNotificationParam, ProgressToken,
        RequestId, ServerJsonRpcMessage,
    };

    use super::{McpDispatchSeam, answered_request};
    use crate::tools::mcp::McpProgressRouter;

    fn id(value: i64) -> RequestId {
        RequestId::Number(value)
    }

    fn token(value: u32) -> ProgressToken {
        ProgressToken(NumberOrString::Number(value.into()))
    }

    fn seam(progress: &Arc<McpProgressRouter>) -> McpDispatchSeam {
        McpDispatchSeam::new(Arc::clone(progress), None)
    }

    /// One outbound `tools/call`, shaped exactly as rmcp hands it to
    /// `Transport::send`: request id and `_meta.progressToken` already set.
    fn tool_call(id: RequestId, token: &ProgressToken) -> ClientJsonRpcMessage {
        let mut request = ClientRequest::CallToolRequest(CallToolRequest::new(
            CallToolRequestParams::new("seam-probe"),
        ));
        request.get_meta_mut().set_progress_token(token.clone());
        JsonRpcMessage::request(request, id)
    }

    /// Both shapes of correlated answer are one rule, so retention can never
    /// become accidentally success-only: a JSON-RPC error ends the remote
    /// request exactly as a result does, and a notification ends nothing.
    #[test]
    fn a_correlated_answer_is_either_a_result_or_an_error() {
        let response: ServerJsonRpcMessage =
            JsonRpcMessage::response(rmcp::model::ServerResult::empty(()), id(1));
        assert_eq!(answered_request(&response), Some(&id(1)));
        let error: ServerJsonRpcMessage = JsonRpcMessage::error(
            ErrorData::internal_error("remote failure", None),
            Some(id(2)),
        );
        assert_eq!(answered_request(&error), Some(&id(2)));
        // A parse-level error the peer could not attribute answers nothing.
        let unattributed: ServerJsonRpcMessage =
            JsonRpcMessage::error(ErrorData::parse_error("unreadable", None), None);
        assert_eq!(answered_request(&unattributed), None);
    }

    /// Neither kind of correlated answer may take pre-subscription evidence
    /// away from a dispatching call that has not claimed it yet, and both
    /// complete the remote dimension so the caller's relinquish forgets the
    /// request.
    #[test]
    fn neither_kind_of_answer_deletes_evidence_the_caller_has_not_claimed() {
        for (index, answer) in [
            JsonRpcMessage::response(rmcp::model::ServerResult::empty(()), id(1)),
            JsonRpcMessage::error(
                ErrorData::internal_error("remote failure", None),
                Some(id(1)),
            ),
        ]
        .into_iter()
        .enumerate()
        {
            let progress = Arc::new(McpProgressRouter::default());
            let seam = seam(&progress);
            seam.admit_progress(&tool_call(id(1), &token(1)))
                .expect("a tools/call is a tracked invocation");
            let lease = progress.lease(id(1), token(1));
            progress.deliver(ProgressNotificationParam::new(token(1), 1.0));
            seam.observe_inbound(&answer);
            assert_eq!(
                progress.pending_occurrences(&id(1)),
                Some(1),
                "answer {index} ends the remote request, not the caller's ownership"
            );
            assert_eq!(
                lease.subscribe().drain().len(),
                1,
                "answer {index} leaves the buffered occurrence for its call to claim"
            );
            drop(lease);
            assert_eq!(
                progress.tracked_requests(),
                0,
                "answer {index} and the caller's relinquish are the forget point"
            );
        }
    }

    /// A request the seam refused to dispatch can never be answered, so the
    /// refusal itself is its remote terminality and it still reaches a
    /// request-local forget point.
    #[test]
    fn a_refused_dispatch_releases_the_progress_state_it_admitted() {
        let progress = Arc::new(McpProgressRouter::default());
        let seam = seam(&progress);
        seam.admit_progress(&tool_call(id(4), &token(4)))
            .expect("a tools/call is a tracked invocation");
        drop(progress.lease(id(4), token(4)));
        assert_eq!(
            progress.tracked_requests(),
            1,
            "the remote side is still open until the seam refuses"
        );
        seam.no_dispatch(&id(4));
        assert_eq!(progress.tracked_requests(), 0);
    }
}
