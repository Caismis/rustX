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

#![allow(clippy::too_many_lines)] // scenario bodies are deliberately linear

/// The fixtures shared with the remaining integration-test binaries.
#[path = "../common/mod.rs"]
pub(crate) mod common;

/// The fixtures that need a `cfg(test)`-only seam.
pub(crate) mod support;

mod issue108_publication;
mod issue109_interaction_audit;
mod issue111_process_death;
mod issue127_todo_plane;
mod issue127_todo_transaction;
mod issue130_agent_status;
mod issue134_model_retry;
mod issue135_model_deadlines;
mod issue136_tool_cancellation_phase;
mod issue137_unresolved_output_carryover;
mod issue138_subagent_conformance;
mod issue140_compaction_metadata;
mod issue27_multi_compaction;
mod issue37_binding;
mod issue37_capability;
mod issue37_endpoint;
mod issue37_protocol;
mod issue37_runtime_client;
mod issue38_conformance;
mod issue38_stdio_transport;
mod issue42_runtime_client_model;
mod issue42_session_model;
mod issue56_lifecycle;
mod issue86_text_spill;
mod m3_agent_loop;
mod m4_context_engine;
mod m5_agent_loop;
mod m5_background;
mod m6_capabilities;
mod native_tool_contracts;
mod tool_progress_bounds;
