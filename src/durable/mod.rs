//! The native durable conversation authority (Issue #11).
//!
//! This module exposes one backend-independent semantic store for the six
//! distinct durable domains. Pending Inbound remains its own authority for
//! accepted-but-not-yet-canonically-adopted work:
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
//! Publication plane      = user-facing release durability (Issue #108)
//! ```
//!
//! Human interaction (Issue #109) deliberately adds **no** seventh domain. A
//! pending Question/Approval waiter is process-owned workflow state and is
//! never made durable; only its low-frequency requested/settled semantic facts
//! are persisted, and those are ordinary Event Journal facts reached through
//! the narrow [`inbox::ConversationInteractionAudit`] capability.
//!
//! The publication plane is the newest and most easily confused of the six. It
//! owns exactly one question — *what did rustX durably commit for release to a
//! user-facing client, and how did that release settle* — and nothing else. It
//! is not the Message Ledger (its content is never conversation history), not
//! the Event Journal (it carries no execution fact), and not a display cache.
//! Its three linearization points are documented on
//! [`crate::publication`].
//!
//! # Backend independence
//!
//! [`inbox::ConversationStore`] declares the backend-independent semantic
//! transitions of all six durable domains. [`sqlite::SqliteConversationStore`]
//! is the M8 concrete backend. A future M11 `PostgreSQL` backend must provide
//! the same observable contract. The abstraction level is deliberately the
//! rustX domain transitions — never a generic repository/queue/CRUD frame.
//!
//! # Two linearization points
//!
//! 1. **Acceptance**: [`ConversationStore::accept_inbound`] commits the sequence
//!    allocation, the pending record, and any correlation/idempotency state
//!    in one transaction. Success is reported only after that commit.
//! 2. **Adoption**: [`ConversationStore::adopt_pending_batch`] atomically
//!    appends the selected pending messages to the durable canonical Message
//!    Ledger, advances the Surface/checkpoint, and removes pending records.
//!
//! The publication plane adds its own three (Issue #108): the provider outcome
//! **P**, the publication terminal **U**
//! ([`ConversationStore::commit_publication_terminal`]), and canonical
//! acceptance **C** ([`ConversationStore::commit_canonical_publication`]),
//! ordered `P < U < C` and enforced by the store as `C => U => P`.

pub mod inbox;
pub mod sqlite;

pub use inbox::{
    AcceptedInbound, CanonicalMessagePage, CompactionCommitInput, ConversationInboundCapability,
    ConversationInteractionAudit, ConversationStore, ConversationStoreBinding,
    ConversationStoreError, DurableConversationHead, EventPage, InboundDraft, PendingBatch,
    PendingInboundItem, RequestSnapshotPage, SurfaceUserMessageBoundary,
    SurfaceUserMessageBoundaryPage, interaction_audit_capability,
};
pub use sqlite::SqliteConversationStore;
