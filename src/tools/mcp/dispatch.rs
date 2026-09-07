//! The rustX-owned outbound dispatch seam (Issue #205).
//!
//! # Why a transport seam, and why exactly here
//!
//! Progress liveness needs a fact rmcp's public request API cannot give a
//! caller. rmcp mints a request's `ProgressToken` *inside*
//! `Peer::send_cancellable_request`, so the dispatching call learns it only
//! after the request was enqueued. Anything that arrives for that token
//! before the call subscribes has no owner, and a router that bounds unowned
//! tokens by capacity can evict a legitimate live request's only liveness
//! evidence — nothing bounds how many admitted requests are inside that
//! window at once.
//!
//! It is answered by owning the request at the outbound transport. rmcp's
//! [`Transport::send`] is called with the fully-formed
//! [`ClientJsonRpcMessage`] — request id and `_meta.progressToken` already
//! set — and its **synchronous prologue runs before the message is handed to
//! the transport at all**. That prologue is therefore a linearization point
//! with a causal, not merely temporal, guarantee:
//!
//! ```text
//! Transport::send prologue        registers (RequestId, ProgressToken)
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

use std::future::Future;
use std::sync::Arc;

use rmcp::model::{
    ClientJsonRpcMessage, ClientRequest, GetMeta as _, ProgressToken, RequestId,
    ServerJsonRpcMessage,
};
use rmcp::service::RoleClient;
use rmcp::transport::Transport;

use super::McpProgressRouter;

/// The transport error this seam reports.
///
/// A transparent wrapper: the seam adds no failure of its own, it only has to
/// name a type of its own to sit between rmcp's service and its transport.
#[derive(Debug)]
pub(crate) struct McpTransportError<E>(E);

impl<E: std::fmt::Display> std::fmt::Display for McpTransportError<E> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

impl<E: std::error::Error + 'static> std::error::Error for McpTransportError<E> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.0)
    }
}

/// The per-connection-generation state the seam owns on behalf of requests.
pub(crate) struct McpDispatchSeam {
    progress: Arc<McpProgressRouter>,
}

impl McpDispatchSeam {
    pub(crate) const fn new(progress: Arc<McpProgressRouter>) -> Self {
        Self { progress }
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
}

fn tool_invocation(message: &ClientJsonRpcMessage) -> Option<ToolInvocation> {
    let ClientJsonRpcMessage::Request(request) = message else {
        return None;
    };
    let ClientRequest::CallToolRequest(call) = &request.request else {
        return None;
    };
    let _ = call;
    Some(ToolInvocation {
        id: request.id.clone(),
        token: request.request.get_meta().get_progress_token(),
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
        let inner = self.inner.send(item);
        let _ = &invocation;
        async move { inner.await.map_err(McpTransportError) }
    }

    async fn receive(&mut self) -> Option<ServerJsonRpcMessage> {
        let message = self.inner.receive().await;
        if let Some(message) = &message {
            self.seam.observe_inbound(message);
        }
        message
    }

    async fn close(&mut self) -> Result<(), Self::Error> {
        self.inner.close().await.map_err(McpTransportError)
    }
}
