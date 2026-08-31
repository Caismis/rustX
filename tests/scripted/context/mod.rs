//! Context engine and compaction pipeline.
//!
//! [`engine`] owns the provider-independent projection/planning semantics:
//! current-Surface projection, token accounting, compaction planning and
//! span selection, complete tool-unit preservation, no resurrection of
//! retired messages, and summary lineage inputs.
//!
//! [`multi_compaction`] and [`compaction_metadata`] own the committed
//! pipeline transition (plan → summarize → validate exact post-summary fit →
//! durable commit → hot-state installation) proved through the final durable
//! `ConversationRuntime` path, including failure atomicity and lineage
//! metadata.

mod compaction_metadata;
mod engine;
mod multi_compaction;
