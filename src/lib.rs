//! rustX runtime library.
//!
//! The crate is intentionally layered around runtime-owned contracts. External
//! SDK types must terminate at adapter boundaries and must not leak into the
//! agent kernel.

pub mod agent;
pub mod capabilities;
pub mod config_format;
pub mod context;
pub mod conversation;
pub mod durable;
pub mod events;
pub mod local_runtime;
pub mod message;
pub mod model;
pub mod protocol;
pub mod publication;
pub mod runtime;
pub mod runtime_client;
pub mod skills;
pub mod tools;

/// The crate's own published name, available to its test build.
///
/// The in-crate suites in [`scripted_suites`] and [`boundary_suites`] are
/// written against the `rustx::` paths an external consumer would use; only
/// their fixtures reach into the `cfg(test)`-only `crate::` seams.
#[cfg(test)]
extern crate self as rustx;

/// The deterministic scripted contract suites.
///
/// They drive the agent loop, the context plane, and the Runtime Client host
/// against a scripted `ModelAdapter` and a scripted `ContextSummarizer`.
/// Both seams are `#[cfg(test)] pub(crate)`, so neither exists in the
/// published API and neither is reachable from an external test binary;
/// the suites therefore compile as part of this crate's test build. Their
/// sources stay under `tests/` so `src/` carries production code only.
///
/// Every suite here is a deterministic contract: no real process, signal,
/// stdio/IPC, or platform-semantics boundary is under test. Suites whose
/// invariant *is* a real operating-system boundary live in
/// [`boundary_suites`] instead, even though they share these `cfg(test)`
/// seams and this lib test binary.
#[cfg(test)]
#[path = "../tests/scripted/mod.rs"]
mod scripted_suites;

/// The in-crate boundary conformance suites.
///
/// These prove contracts whose invariant is a real operating-system or
/// runtime boundary — process death, child-process supervision, real stdio
/// fixtures, shell background execution. They compile into the same lib test
/// binary because they need the same `#[cfg(test)] pub(crate)` seams, but
/// they are boundary conformance tests, not deterministic contracts, and
/// CI selects them by their stable `boundary_suites::` name prefix.
#[cfg(test)]
#[path = "../tests/boundary/mod.rs"]
mod boundary_suites;
