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

    /// Takes dispatch ownership of one tool invocation, immediately before
    /// its message is handed to the transport.
    ///
    /// Deliberately as late as possible: every cancellation that lands
    /// before this point gets the clean
    /// [`crate::tools::mcp::streamable_http::LocalRequestTermination::PreDispatchTerminated`]
    /// outcome — nothing local began, and the refusal here guarantees
    /// nothing local ever will — rather than a settlement that has to wait
    /// for a local owner it did not need to create.
    fn begin_dispatch(&self, id: &RequestId) -> OutboundDispatch {
        self.ownership.as_ref().map_or_else(
            || OutboundDispatch::Owned(DispatchOwnership::none()),
            |ownership| ownership.begin_dispatch(id),
        )
    }

    /// The inbound terminal-correlation point.
    ///
    /// A correlated response is the terminal forget point of the progress
    /// state of a request whose dispatching call is gone. A request whose
    /// call still holds its subscription keeps its state until that guard
    /// drops.
    fn observe_inbound(&self, message: &ServerJsonRpcMessage) {
        if let Some(id) = answered_request(message) {
            self.progress.settle(id);
        }
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

impl<T> Transport<RoleClient> for ObservingTransport<T>
where
    T: Transport<RoleClient> + Send,
{
    type Error = McpTransportError<T::Error>;

    fn send(
        &mut self,
        item: ClientJsonRpcMessage,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send + 'static {
        // Synchronous, and before `item` reaches the transport: this is the
        // progress-token linearization point the whole module exists for.
        let invocation = self.seam.admit_progress(&item);
        // The inner send future is constructed here because it needs the
        // transport, but it is not polled until the dispatch is owned below,
        // so nothing has been handed to the wire yet.
        let inner = self.inner.send(item);
        let seam = Arc::clone(&self.seam);
        async move {
            let Some(invocation) = invocation else {
                // Not a tool invocation: no settlement can ever terminate it,
                // so it owns nothing and passes straight through.
                return inner.await.map_err(McpTransportError::Transport);
            };
            // Test-only: holds one tool invocation between its admission and
            // its dispatch ownership, which is the window in which a
            // cancellation is provably pre-dispatch. No production path
            // installs a pause.
            #[cfg(test)]
            crate::tools::mcp::test_sync::park_before_outbound_dispatch(&invocation.tool).await;
            let guard = match seam.begin_dispatch(&invocation.id) {
                OutboundDispatch::Owned(guard) => guard,
                OutboundDispatch::Refused => {
                    // The tool call was already settled. Dropping the inner
                    // send future without polling it is what keeps the
                    // request off the network entirely.
                    drop(inner);
                    return Err(McpTransportError::LocallyTerminated);
                }
            };
            // Dispatch ownership lives exactly as long as this send does,
            // unless the POST takes the baton first.
            let _guard = guard;
            inner.await.map_err(McpTransportError::Transport)
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
        self.inner
            .close()
            .await
            .map_err(McpTransportError::Transport)
    }
}
