//! Subagent boundary tests.
//!
//! A subagent child is an ordinary child `ConversationRuntime`: generic
//! retry, deadline, cancellation, publication, and settlement semantics are
//! owned by the Agent Loop suites and are **not** re-proven here. This
//! target owns only what the subagent boundary adds:
//!
//! - named child-definition admission, resolution, and the frozen child
//!   specification ([`definitions`]);
//! - the real child-process handshake: a launched named child consumes the
//!   frozen definition and policy through the typed spawn path
//!   ([`process_conformance`]);
//! - the end-to-end parent/child composition through the real binary over
//!   the stdio/JSONL transport ([`end_to_end`]).

#![allow(clippy::too_many_lines)] // deterministic scenario bodies stay linear

#[path = "../common/mod.rs"]
mod common;

mod definitions;
mod end_to_end;
mod process_conformance;
