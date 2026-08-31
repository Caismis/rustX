//! SQLite durability and recovery boundary tests.
//!
//! Every suite here speaks to the real file-backed durable authority. The
//! crash boundary is a store `drop` and the reopen is a
//! `SqliteConversationStore::open`; recovery classification runs the real
//! recovery pipeline. There is no sleep, no timer, and no timing assumption:
//! an exact committed prefix is the synchronization.
//!
//! These suites prove the store/recovery contract at the SQLite boundary.
//! The Agent-Loop-facing halves of the same invariants (publication
//! coalescing, interaction settlement, terminal uniqueness) are owned by the
//! in-crate scripted suites and are not re-proven here.

#![allow(clippy::too_many_lines)] // deterministic store scenarios stay linear

mod interaction_audit;
mod pending_inbound;
mod publication;
mod recovery;
mod transcript_history;
