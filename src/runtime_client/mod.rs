//! The Runtime Client boundary (Issue #37): the one external observation
//! boundary of rustX Runtime state.
//!
//! # Architecture
//!
//! ```text
//! canonical runtime state / internal RuntimeEvent
//!                 |
//!                 v
//!  deterministic Runtime Client projection
//!                 |
//!                 v
//!  RuntimeClientEvent / RuntimeClientSnapshot
//!                 |
//!                 v
//!       Runtime Client Protocol v1
//! ```
//!
//! The governing invariant:
//!
//! > All authoritative execution and conversation state originates from
//! > rustX Runtime. External clients observe projections of that state;
//! > they never become a second authority.
//!
//! The internal [`RuntimeEvent`](crate::events::types::RuntimeEvent)
//! vocabulary is an execution-fact vocabulary, **not** the wire contract.
//! [`RuntimeClientEvent`](event::RuntimeClientEvent) and
//! [`RuntimeClientSnapshot`](snapshot::RuntimeClientSnapshot) are
//! explicit runtime-owned projection types with their own versioning,
//! lifecycle semantics, and cursor domain. Later transports (Issue #38
//! stdio JSONL, Issue #36 WebSocket) wrap this semantic layer without
//! redefining it, and an AG-UI adapter consumes this projection as its
//! only source — there is no second AG-UI interpretation path directly
//! from internal runtime events.
//!
//! # Ownership summary
//!
//! - [`RuntimeClientEndpoint`](endpoint::RuntimeClientEndpoint) is the
//!   semantic protocol entry point: it dispatches every v1 request,
//!   `initialize` included, so a transport stays a framing adapter and
//!   never owns negotiation or attachment admission.
//! - [`RuntimeClientHost`](host::RuntimeClientHost) coordinates
//!   admission and the current-attempt handle; `AgentExecution` remains
//!   the attempt settlement authority.
//! - [`RuntimeClientProjection`](projection::RuntimeClientProjection) is
//!   the one linearization owner of the externally visible read model,
//!   cursor allocation, event publication, bounded replay, and
//!   subscribers.
//! - The canonical mailbox, background registry, and capability
//!   coordinator remain authoritative; the projection observes them
//!   through narrow read-only seams.
//! - Agent Status is composed exactly once per request preparation; the
//!   model path and the client projection consume the same composed
//!   observation.
//!
//! # Protocol v1 scope
//!
//! - one active attachment per runtime instance;
//! - detach is never cancellation;
//! - bounded in-memory replay (no Event Journal, no persistence, no
//!   crash-safe resume claims — M8 owns durability);
//! - no WebSocket (Issue #36), no TUI (Issue #39), no M9 cancellation
//!   hierarchy, no AG-UI adapter implementation.
//!
//! # Transports
//!
//! [`transport`] holds the byte-stream adapters beneath the semantic
//! layer — [`transport::stdio`] is the strict stdio/JSONL transport of
//! Issue #38. A transport frames; it never re-implements semantics, and
//! transport loss detaches without cancelling or settling anything.

pub mod attachment;
pub mod endpoint;
pub mod event;
pub mod host;
pub mod projection;
pub mod snapshot;
pub mod transport;
pub mod types;

#[cfg(test)]
mod test_sync;

pub use attachment::RuntimeAttachment;
pub use endpoint::RuntimeClientEndpoint;
pub use event::{RuntimeClientAttemptFailure, RuntimeClientEvent, RuntimeClientOutcome};
pub use host::{
    EventDelivery, EventSubscription, HostConstructionError, RuntimeClientContextConfig,
    RuntimeClientHost, RuntimeClientHostConfig,
};
pub use snapshot::{
    AgentStatusView, CapabilityView, ForegroundToolExecution, ForegroundToolState,
    InFlightAgentMessage, InFlightBlock, InboundDiagnostics, InboundDrainView, InboundItemView,
    RuntimeClientAttempt, RuntimeClientAttemptPhase, RuntimeClientBackgroundExecution,
    RuntimeClientSkill, RuntimeClientSnapshot, RuntimeClientStatusFact, RuntimeClientStatusSection,
    RuntimeClientTool,
};
pub use types::{
    AttachmentId, RUNTIME_CLIENT_PROTOCOL_VERSION_V1, RequestId, RuntimeClientCursor,
    RuntimeClientError, RuntimeClientProtocolEvent, RuntimeClientRequest, RuntimeClientResponse,
    RuntimeClientResult,
};
