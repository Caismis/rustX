//! The durable user-facing publication plane (Issue #108, FND-03).
//!
//! Publication is a **separate durable plane** from provider outcome and from
//! canonical conversation acceptance. It exists to hold exactly one
//! user-facing contract:
//!
//! > No semantic output is released to a user-facing Runtime Client before
//! > rustX has durably committed that publication.
//!
//! # Three linearization points
//!
//! Any request that emits user-facing model output has three distinct commit
//! points, owned by three distinct planes:
//!
//! ```text
//! P — Provider outcome        ModelRequestCompleted durable   (Event Journal)
//! U — Publication outcome     final frame + terminal marker   (publication plane)
//! C — Conversation acceptance canonical Assistant durable     (Message Ledger)
//! ```
//!
//! The required commit ordering is `P < U < C`, and the durable store — not
//! only Agent Loop control flow — enforces the implication `C => U => P`.
//! P and U are deliberately never combined into one transaction: "the
//! provider finished" and "rustX committed this output for release" are
//! different facts, and a crash between them must stay distinguishable.
//!
//! # Pipeline
//!
//! Provider chunk size is not the publication unit:
//!
//! ```text
//! Provider ModelEvent delta
//!   -> in-memory assembler          (canonical message assembly)
//!   -> bounded publication coalescer(bytes / oldest-deadline latency / structure / terminal)
//!   -> typed publication frame
//!   -> durable publication staging
//!   -> user-facing release
//! ```
//!
//! [`PublicationCoalescer`] owns the bounded deterministic flush policy. When
//! the first payload enters an empty buffer it owns one absolute deadline;
//! later provider events never reset it. The runtime [`MonotonicClock`] owns
//! the wake-up mechanism, so deterministic tests use
//! [`ManualMonotonicClock`](crate::runtime::ManualMonotonicClock) and never a
//! sleep.
//!
//! # Three mutually exclusive settlements
//!
//! One publication stream settles exactly once:
//!
//! ```text
//! Canonical                 U reached, C reached — the Ledger is the authority
//! UnacceptedPublicationAudit U reached, C never  — complete output, never accepted
//! IncompletePublicationAudit U never reached     — publication has no durable terminal
//! ```
//!
//! Incomplete is defined on the **publication** boundary, never on the
//! provider boundary:
//!
//! > Incomplete Publication means user-facing publication did not reach its
//! > own durable terminal boundary. It does not imply that the provider
//! > necessarily failed to reach transport termination.
//!
//! So a stream whose `ModelRequestCompleted` is durable but whose U never
//! committed is Incomplete, and a structural `assembler.finish()` rejection
//! after frames were already released is Incomplete (no P exists at all).
//!
//! # Audit semantics
//!
//! A [`PublicationAudit`] records the semantic output rustX durably committed
//! **for release**. It is an upper bound on what may have been displayed and
//! never proof of perception; rustX adds no Runtime Client ACK protocol.
//!
//! An audit never becomes canonical Assistant content, acquires Message
//! Ledger or Surface identity, enters lineage meaning, or becomes a generic
//! part of future model context. The sole narrow exception is a bounded,
//! explicitly request-only projection of a terminally unresolved audit: one
//! selected source may be frozen by value into exactly one later eligible
//! primary request (and its `RequestSnapshot`), with no fabricated
//! `MessageId`. See [`carryover`] for the shared selector and renderer.
//!
//! # Tool proposals are not executions
//!
//! Tool-call frames are **model proposals**, never Tool Plane execution
//! facts. The vocabulary names them so
//! ([`PublicationPayload::ProposedToolCallStarted`] and siblings), and the
//! durable store enforces the hard invariant that no proposal belonging to an
//! Incomplete or Unaccepted publication may have a dependent
//! `ToolExecutionStarted`, `ToolResult`, or side-effect authorization.
//! The store also owns the proposal staging state machine: Started freezes the
//! stream-local block/tool/name identity, arguments require Started, and
//! Completion advances it exactly once to Completed.

pub mod carryover;
pub mod coalescer;
pub mod frame;

pub use carryover::{
    MAX_CARRYOVER_TEXTUAL_BLOCK_BYTES, MAX_CARRYOVER_TOOL_ARGUMENT_BYTES,
    MAX_UNRESOLVED_OUTPUT_CARRYOVER_BYTES, render_unresolved_output_carryover,
    select_unresolved_output_source,
};
pub use coalescer::{CoalescePolicy, PublicationCoalescer};
pub use frame::{
    PublicationAudit, PublicationAuditBlock, PublicationAuditKind, PublicationFrame,
    PublicationPayload, PublicationSettlement, PublicationStreamRecord, PublicationStreamStart,
    consolidate_audit_content,
};
