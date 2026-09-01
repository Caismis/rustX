//! Agent Loop: the owner of generic execution semantics.
//!
//! This domain authoritatively owns the cross-module execution contracts:
//! the attempt state machine, terminal uniqueness and terminal-last
//! ordering, `AttemptOutcome`/terminal-fact correspondence, the model
//! request start/settlement lifecycle with exact request counts and
//! ordinals, canonical Assistant commit rules, the generic tool lifecycle
//! with deterministic canonical result ordering, cancellation arbitration
//! and structural settlement, transient retry with frozen replay, model
//! deadlines, unresolved-output carryover, and the publication interaction
//! at the Agent Loop boundary.
//!
//! Feature suites in other domains assert these contracts only where they
//! cross that feature's boundary; they never re-prove the state machines
//! owned here.

mod carryover;
mod deadlines;
mod execution;
mod lifecycle;
mod publication;
mod retry;
mod status;
mod stream_progress;
mod tool_cancellation;
mod tool_progress;
mod tool_scheduling;
