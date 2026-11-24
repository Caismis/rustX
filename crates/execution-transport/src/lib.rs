//! Transport mechanics for the rustX execution boundary.
//!
//! One [`RecordChannel`] carries exactly one `EncodedRecord` per exchange:
//! a length-prefixed frame on a Unix socket, or exactly one binary
//! WebSocket message. This crate owns transport mechanics only —
//! deadline-enforcing socket adapters, strict endpoint validation, TLS,
//! and the RFC 6455 handshake. Protocol DTOs, authentication, and
//! admission policy stay in `rustx-execution-protocol` and
//! `rustx-execution-server`; this layer never interprets record bodies.
//!
//! The WebSocket wire envelope (RFC 6455 frames, per-message boundaries,
//! and TLS record framing on `wss`) is distinct from the application
//! envelope: the bounded JSON `EncodedRecord` produced by
//! `rustx_execution_protocol::transport::encode_record` is the complete
//! payload of one binary message. No nested UDS length prefix is
//! transmitted. RFC control frames (Ping/Pong/Close) are transport-level
//! liveness only — they never substitute for the protocol's independent
//! execution controls, and a control-frame flood is bounded by the same
//! absolute deadline as record traffic.
//!
//! Supported endpoint forms are deliberately narrow: `ws://IP:PORT` is
//! accepted only for a literal loopback IP, `wss://IP:PORT` accepts any
//! literal IP, the path must be exactly `/execution/v8`, and userinfo,
//! query, and fragment are rejected. Hostnames are unsupported — numeric
//! IPs keep resolution inside the bounded connect budget and pair with
//! IP-SAN certificates. `wss` certificate and hostname verification can
//! never be disabled; a private deployment supplies extra trust anchors
//! only through an explicit operator CA file.

mod channel;
mod deadline;
mod endpoint;
mod path;

pub use channel::{ChannelShutdown, RecordChannel};
pub use endpoint::ExecutionUrl;
pub use path::open_dir_nofollow;

pub use channel::{LoopbackTls, accept, connect, connect_provisioned};

pub(crate) use rustx_execution_protocol::EXECUTION_PATH;

#[cfg(test)]
mod tests;
