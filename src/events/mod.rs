//! Durable runtime event types and event-writer abstractions.
//!
//! M1 defines the canonical event envelope and initial event vocabulary.
//! Event writers, persistence, and external publication are milestone M8.

pub mod types;

pub use types::{
    AttemptFailure, AttemptLimit, AttemptOutcome, EVENT_SCHEMA_VERSION, RuntimeEvent,
    RuntimeEventEnvelope,
};
