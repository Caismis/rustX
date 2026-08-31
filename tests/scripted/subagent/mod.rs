//! Subagent: the child ownership boundary.
//!
//! A subagent child is an ordinary child `ConversationRuntime`. Generic
//! retry, deadline, cancellation, publication, settlement, and carryover
//! semantics are owned by the [`super::agent`] suites and are deliberately
//! not re-proven here. This domain owns only what the boundary adds: frozen
//! child authority crossing into the child, parent registry lifecycle,
//! exactly one terminal child notice, parent isolation from child-internal
//! retry/deadline state, and cancellation/drain across the ownership
//! boundary.
//!
//! The real-process half of the boundary (a launched named child consuming
//! the frozen definition through the typed spawn path) lives in the external
//! `subagent` integration target.

mod conformance;
