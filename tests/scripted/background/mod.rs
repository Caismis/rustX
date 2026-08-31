//! Background execution and the model-facing execution control plane.
//!
//! Owns the conversation-owned background registry contracts (deterministic
//! `exec_N` allocation, two-stage dispatch ownership, cancel-vs-completion
//! linearization, exactly-once terminal settlement, bounded progress) and
//! the unified `execution` intrinsic surface for detached tool executions
//! and subagent children.

mod execution_intrinsic;
mod registry;
mod text_spill;
