//! Durable runtime event types, the event-writer abstraction, and the narrow
//! non-durable execution-fact sink.
//!
//! M1 defines the canonical event envelope and initial event vocabulary.
//! Event writers, persistence, and external publication are milestone M8.
//! M5 adds a narrow non-durable [`RuntimeEventSink`] so detached background
//! executions can emit execution facts after their originating attempt
//! ended; durable Event Journal writing remains M8.

pub mod types;

/// The narrow non-durable execution-fact sink of the tool plane.
///
/// Detached background executions emit facts (for example progress events)
/// through this seam after their originating attempt may have ended. The
/// sink is explicitly not the M8 durable Event Journal: it performs no
/// sequence allocation, no persistence, and no ordering beyond best-effort
/// in-memory delivery. No second competing event history exists — the
/// canonical [`RuntimeEvent`] vocabulary is unchanged.
pub trait RuntimeEventSink: Send + Sync {
    /// Emits one execution fact.
    fn emit(&self, event: RuntimeEvent);
}

/// A recording in-memory sink for deterministic tests.
///
/// The sink records every emitted event; this is a development/test seam,
/// never a durable store.
#[derive(Debug, Default, Clone)]
pub struct RecordingEventSink {
    events: std::sync::Arc<std::sync::Mutex<Vec<RuntimeEvent>>>,
}

impl RecordingEventSink {
    /// Creates an empty recording sink.
    #[must_use]
    pub fn new() -> Self {
        Self {
            events: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
        }
    }

    /// The recorded events in emission order.
    ///
    /// # Panics
    ///
    /// Panics only if the recording lock is poisoned.
    #[must_use]
    pub fn events(&self) -> Vec<RuntimeEvent> {
        self.events.lock().expect("recording sink lock").clone()
    }
}

impl RuntimeEventSink for RecordingEventSink {
    fn emit(&self, event: RuntimeEvent) {
        self.events.lock().expect("recording sink lock").push(event);
    }
}

pub use types::{
    AttemptFailure, AttemptLimit, AttemptOutcome, EVENT_SCHEMA_VERSION, RuntimeEvent,
    RuntimeEventEnvelope,
};
