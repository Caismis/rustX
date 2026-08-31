//! The deterministic scripted regression suites, compiled into the crate's
//! own test build.
//!
//! # Why these suites are in-crate
//!
//! Every suite below needs one of exactly two seams:
//!
//! - a scripted [`ModelAdapter`](rustx::model::ModelAdapter) behind a real
//!   catalog binding, so the agent loop, the context plane, and the Runtime
//!   Client host can be driven turn by turn without a provider;
//! - a scripted [`ContextSummarizer`](rustx::context::ContextSummarizer)
//!   behind a real context runtime, so compaction can be driven without a
//!   summary model.
//!
//! Neither seam may exist in the published API: production must have exactly
//! one binding path ([`ModelBindingRegistry::new`](rustx::model::ModelBindingRegistry::new),
//! which constructs the three supported protocol adapters directly) and
//! exactly one production context-runtime construction path
//! ([`ContextRuntime::for_attempt_with_assembly`](rustx::context::ContextRuntime::for_attempt_with_assembly),
//! which derives the summarizer from the attempt's frozen model snapshot and
//! requires the admitted timeout policy and shared monotonic clock). The
//! `cfg(test)` `for_attempt` wrapper only supplies an empty assembly for
//! these in-crate fixtures; it also requires those explicit admitted values.
//!
//! An external integration-test binary can only reach `pub` items, so a seam
//! usable from `tests/*.rs` is necessarily a seam a downstream consumer can
//! call. `#[doc(hidden)]` hides such a seam from documentation; it does not
//! remove it. These suites therefore compile as part of the crate's own test
//! build, where both seams are `#[cfg(test)] pub(crate)` and unreachable
//! from any consumer of the library.
//!
//! # Layout
//!
//! The sources stay under `tests/` so `src/` contains production code only —
//! the source-level guards that scan `src/model`, `src/agent`, and
//! `src/context` keep their exact meaning. Cargo auto-discovers integration
//! targets from `tests/*.rs` and `tests/*/main.rs` only, so nothing here is
//! also built as a separate test binary.
//!
//! The suites are written against the published `rustx::` paths, exactly as
//! an external consumer would write them; only [`support`] reaches into the
//! `cfg(test)`-only `crate::` seams.
//!
//! # Domain ownership
//!
//! Suites are grouped by the runtime layer that owns the invariant under
//! test, never by the issue or milestone that introduced them:
//!
//! - [`agent`] — generic execution semantics: the attempt state machine,
//!   settlement/terminal contracts, request lifecycle and ordinals,
//!   canonical commit rules, tool lifecycle and ordering, cancellation
//!   arbitration, retry, deadlines, carryover, and publication interaction.
//! - [`context`] — provider-independent projection/planning and the
//!   committed compaction pipeline transition.
//! - [`runtime_client`] — the host/endpoint/protocol/transport contracts.
//! - [`capability`] — capability snapshots, quiescent commits,
//!   materialization.
//! - [`interaction`] — the durable interaction audit's runtime half.
//! - [`background`] — the background registry and the `execution`
//!   intrinsic control plane.
//! - [`subagent`] — the child ownership boundary only; generic execution
//!   semantics stay in [`agent`].
//! - [`tools`] — native registry contracts and the conversation task list.
//! - [`durable`] — real process-death conformance over the durable
//!   authority.
//!
//! See `tests/README.md` for the full test architecture.

#![allow(clippy::too_many_lines)] // scenario bodies are deliberately linear

/// The fixtures shared with the remaining integration-test binaries.
#[path = "../common/mod.rs"]
pub(crate) mod common;

/// The fixtures that need a `cfg(test)`-only seam.
pub(crate) mod support;

mod agent;
mod background;
mod capability;
mod context;
mod durable;
mod interaction;
mod runtime_client;
mod subagent;
mod tools;
