//! The durable Pending Inbound Inbox (Issue #63).
//!
//! This module owns the one durable authority for accepted-but-not-yet
//! canonically-adopted inbound work:
//!
//! ```text
//! Pending Inbound Inbox
//!     owns accepted / not-yet-adopted inbound durability
//!     owns the one per-conversation InboundSequence allocator
//!     owns the acceptance linearization point
//!     participates in the canonical-adoption transaction
//! ```
//!
//! It is **not** the Message Ledger, not the Conversation Surface, not the
//! Event Journal, not a generic queue framework, and not a second scheduler.
//! The semantic ownership split is:
//!
//! ```text
//! Pending Inbound Inbox  = accepted / not-yet-adopted inbound durability
//! ConversationInboundMailbox = process-local coordination / wakeup
//! Message Ledger         = adopted canonical conversational facts
//! Conversation Surface   = current model-visible ordering/projection
//! ConversationRuntime    = admission + safe-boundary adoption owner
//! Event Journal          = execution facts
//! ```
//!
//! # Backend independence
//!
//! [`inbox::InboundStore`] declares the backend-independent domain semantic
//! operations (accept, select, adopt, load). [`sqlite::SqliteInboundStore`]
//! is the M8 concrete backend. A future M11 `PostgreSQL` backend must provide
//! the same observable contract. The abstraction level is deliberately the
//! rustX domain transitions — never a generic repository/queue/CRUD frame.
//!
//! # Two linearization points
//!
//! 1. **Acceptance**: [`InboundStore::accept_inbound`] commits the sequence
//!    allocation, the pending record, and any correlation/idempotency state
//!    in one transaction. Success is reported only after that commit.
//! 2. **Adoption**: [`InboundStore::adopt_pending_batch`] atomically appends
//!    the selected pending messages to the durable canonical message ledger
//!    and removes the pending records in the same transaction.

pub mod inbox;
pub mod sqlite;

pub use inbox::{
    AcceptedInbound, InboundDraft, InboundStore, InboundStoreError, PendingBatch,
    PendingInboundItem,
};
pub use sqlite::SqliteInboundStore;
