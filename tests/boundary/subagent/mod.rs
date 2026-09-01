//! Boundary conformance for the subagent child boundary: real staged child
//! processes (`sh`, own process group), a real unix control socket, and the
//! registry's OS-level lifecycle.
//!
//! Generic execution semantics (retry/deadline/cancellation state machines)
//! are *not* re-proven here; they belong to `scripted_suites::agent`. Only
//! contracts whose invariant crosses the real child-process boundary live
//! here.

mod conformance;
mod execution_routing;
