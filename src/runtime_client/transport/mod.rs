//! Runtime Client transports: bounded byte-stream adapters around the
//! semantic endpoint (Issue #38).
//!
//! # Layering
//!
//! ```text
//! rustX Runtime
//!       |
//!       v
//! Runtime Client projection
//!       |
//!       v
//! Runtime Client Protocol v3        (semantic; Issue #37)
//!       |
//!       v
//! transport adapters                (framing only; this module)
//!       |
//!       +-- stdio / strict JSONL    (Issue #38)
//!       |
//!       +-- WebSocket               (Issue #36, later)
//!       |
//!       v
//! clients
//! ```
//!
//! Everything under this namespace is framing, I/O ordering, bounded
//! buffering, and local session termination. Nothing here is semantic: a
//! transport calls
//! [`RuntimeClientEndpoint::handle_request`](super::endpoint::RuntimeClientEndpoint::handle_request)
//! and forwards
//! [`EventSubscription`](super::host::EventSubscription) deliveries, and it
//! implements no protocol-version negotiation, no attachment admission, no
//! [`AttachmentId`](super::types::AttachmentId) allocation, no snapshot,
//! cancellation, replay, or shutdown semantics.
//!
//! The two governing transport invariants:
//!
//! > Only a complete, valid, in-bound-size framed Runtime Client request
//! > may cross into `RuntimeClientEndpoint::handle_request`.
//!
//! > Transport loss detaches the endpoint but never synthesizes semantic
//! > cancellation, settlement, mailbox mutation, or canonical-history
//! > mutation.
//!
//! A transport owns no event backlog. The Runtime Client projection's
//! bounded replay ring remains the one retained Runtime Client event
//! backlog, and a stalled transport consumer costs one cursor rather than a
//! growing queue.
//!
//! Adding a transport (Issue #36 WebSocket) means adding a sibling module
//! here; no semantic module moves, and the transport-independent
//! conformance scenarios apply unchanged.

pub mod stdio;

pub use stdio::{
    STDIO_JSONL_MAX_RECORD_BYTES, STDIO_JSONL_READ_CHUNK_BYTES, StdioFramingError, StdioSessionEnd,
    StdioTransportError, serve_stdio_jsonl, serve_stdio_jsonl_with_io,
};
