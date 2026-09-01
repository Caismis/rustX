//! Context engine, compaction pipeline, and runtime integration.
//!
//! Ownership is layered deliberately:
//!
//! - [`engine`] owns the provider-independent `ContextEngine` contracts:
//!   current-Surface projection, token accounting, compaction planning and
//!   span selection, complete tool-unit preservation, no resurrection of
//!   retired messages, and summary lineage inputs. Tests drive
//!   `ContextEngine` / `ConversationState` directly.
//! - [`compaction_pipeline`] owns the shared committed transition — plan →
//!   summarize → validate exact post-summary fit → durable commit →
//!   hot-state installation — driven through the real `execute_compaction`
//!   implementation against a real `SQLite` store, including the
//!   failure-atomicity matrix, plus the adapter-backed summarizer contract
//!   (the summarize stage).
//! - [`compaction_metadata`] owns the structured summary metadata/lineage
//!   extraction contracts at engine level.
//! - [`runtime_integration`] owns `AgentExecution` ↔ context composition:
//!   proactive compaction invocation, overflow compact-and-retry, failure
//!   classification at the Agent Loop boundary, cancellation, continuation
//!   invalidation, and drained-inbound interaction. It asserts observable
//!   boundary contracts and relies on the lower-layer owners for internal
//!   semantics.
//! - [`runtime_multi_compaction`] owns multi-attempt `ConversationRuntime`
//!   composition: historical request reconstruction, Runtime Client
//!   detach/reattach, continuation ownership across attempts, and the frozen
//!   session summary model.

mod compaction_metadata;
mod compaction_pipeline;
mod engine;
mod runtime_integration;
mod runtime_multi_compaction;
