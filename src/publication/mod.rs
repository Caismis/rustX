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
//!   -> bounded publication coalescer(bytes / latency / structure / terminal)
//!   -> typed publication frame
//!   -> durable publication staging
//!   -> user-facing release
//! ```
//!
//! [`PublicationCoalescer`] owns the bounded deterministic flush policy.
//! Latency flushing reads a [`PublicationClock`], so deterministic tests use
//! [`ManualPublicationClock`] and never a sleep.
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
//! An audit never enters the Message Ledger, the active Surface, a
//! `RequestSnapshot`, a tree/fork/clone seed, or any future model request.
//!
//! # Tool proposals are not executions
//!
//! Tool-call frames are **model proposals**, never Tool Plane execution
//! facts. The vocabulary names them so
//! ([`PublicationPayload::ProposedToolCallStarted`] and siblings), and the
//! durable store enforces the hard invariant that no proposal belonging to an
//! Incomplete or Unaccepted publication may have a dependent
//! `ToolExecutionStarted`, `ToolResult`, or side-effect authorization.

pub mod coalescer;
pub mod frame;

pub use coalescer::{
    CoalescePolicy, ManualPublicationClock, PublicationClock, PublicationCoalescer,
    SystemPublicationClock,
};
pub use frame::{
    PublicationAudit, PublicationAuditBlock, PublicationAuditKind, PublicationFrame,
    PublicationPayload, PublicationSettlement, PublicationStreamRecord, PublicationStreamStart,
    consolidate_audit_content,
};
