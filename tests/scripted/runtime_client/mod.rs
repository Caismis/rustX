//! Runtime Client: the host, endpoint, protocol, and transport contracts.
//!
//! The host owns canonical history, the current-attempt slot, projections
//! and cursor domains, attachment state, and identity counters. The
//! transport-independent conformance matrix runs one scenario set through
//! the direct endpoint and the stdio/JSONL framing; byte-level framing
//! regressions live in [`stdio_transport`].

mod binding;
mod conformance;
mod endpoint;
mod host;
mod model;
mod protocol;
mod session_model;
mod stdio_transport;
