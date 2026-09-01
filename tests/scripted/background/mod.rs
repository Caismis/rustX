//! Background execution and the model-facing execution control plane.
//!
//! Owns the conversation-owned background registry contracts (deterministic
//! `exec_N` allocation, two-stage dispatch ownership, cancel-vs-completion
//! linearization, exactly-once terminal settlement, bounded progress) and
//! the deterministic half of the unified `execution` intrinsic surface
//! (routing for detached tool executions).
//!
//! All suites here are deterministic contracts driven by scripted
//! executions. Real bash background execution (spill, terminal inbound)
//! is boundary conformance and lives in `boundary_suites::background`;
//! `execution(status|cancel)` routing against real staged subagent children
//! lives in `boundary_suites::subagent::execution_routing`.

mod execution_intrinsic;
mod registry;
