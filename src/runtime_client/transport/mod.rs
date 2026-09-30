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
//! Runtime Client protocol        (semantic; Issue #37/#130/#136)
//!       |
//!       v
//! transport adapters                (framing only; this module)
//!       |
//!       +-- strict JSONL framing    (Issue #38)
//!       |
//!       v
//! child live inspection socket (read-only)
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
//! This binding is internal: its one production consumer is the child-owned,
//! read-only live inspection socket. External clients use App Server, whose
//! stdio JSONL and WebSocket transports bind `AppServerConnection`, not this
//! endpoint (shared parity scenario: `tests/support/app_server_conformance.rs`).

pub mod stdio;

pub use stdio::{
    STDIO_JSONL_MAX_RECORD_BYTES, STDIO_JSONL_READ_CHUNK_BYTES, StdioFramingError, StdioSessionEnd,
    StdioTransportError, serve_stdio_jsonl_with_io,
};
