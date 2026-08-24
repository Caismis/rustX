//! rustX runtime library.
//!
//! The crate is intentionally layered around runtime-owned contracts. External
//! SDK types must terminate at adapter boundaries and must not leak into the
//! agent kernel.

pub mod agent;
pub mod capabilities;
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
/// The scripted suites in [`scripted_suites`] are written against the
/// `rustx::` paths an external consumer would use; only their fixtures reach
/// into the `cfg(test)`-only `crate::` seams.
#[cfg(test)]
extern crate self as rustx;

/// The deterministic scripted regression suites.
///
/// They drive the agent loop, the context plane, and the Runtime Client host
/// against a scripted `ModelAdapter` and a scripted `ContextSummarizer`.
/// Both seams are `#[cfg(test)] pub(crate)`, so neither exists in the
/// published API and neither is reachable from an external test binary;
/// the suites therefore compile as part of this crate's test build. Their
/// sources stay under `tests/` so `src/` carries production code only.
#[cfg(test)]
#[path = "../tests/scripted/mod.rs"]
mod scripted_suites;
