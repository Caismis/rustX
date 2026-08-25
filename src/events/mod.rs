//! Durable runtime event types and the narrow optional non-durable execution-
//! fact sink.
//!
//! M1 defines the canonical event envelope and initial event vocabulary. M8
//! persists the envelope in the `ConversationStore` Event Journal before
//! publication. M5's [`RuntimeEventSink`] remains an optional process-local
//! progress projection for detached background work; it is not a second
//! durable journal.

pub mod interaction;
pub mod types;

/// The narrow optional non-durable execution-fact projection of the tool
/// plane.
///
/// Detached background executions emit facts (for example progress events)
/// through this seam after their originating attempt may have ended. The
/// sink is explicitly not the `ConversationStore` Event Journal: it performs no
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

pub use interaction::{
    CustomAnswer, InteractionSettlement, InteractionSubject, MAX_APPROVAL_DENIAL_REASON_CHARS,
    MAX_APPROVAL_REQUEST_REASON_CHARS, MAX_APPROVAL_TOOL_NAME_CHARS, MAX_CUSTOM_ANSWER_CHARS,
    MAX_OPTION_DESCRIPTION_CHARS, MAX_OPTION_LABEL_CHARS, MAX_OPTION_PREVIEW_CHARS,
    MAX_QUESTION_HEADER_CHARS, MAX_QUESTION_TEXT_CHARS, MAX_QUESTIONNAIRE_OPTIONS,
    MAX_QUESTIONNAIRE_QUESTIONS, MIN_QUESTIONNAIRE_OPTIONS, MultipleOptionAnswer,
    OptionSpecification, QuestionSpecification, QuestionnaireAnswer, QuestionnaireAnswerEntry,
    QuestionnaireDeclined, QuestionnaireResponse, QuestionnaireSpecification,
    QuestionnaireSubmission, SingleOptionAnswer, interaction_arguments_digest,
    normalize_questionnaire_response, normalize_questionnaire_submission,
    validate_interaction_settlement, validate_interaction_subject, validate_questionnaire,
};
pub use types::{
    AttemptFailure, AttemptLimit, AttemptOutcome, BackgroundTerminalState, EVENT_SCHEMA_VERSION,
    RuntimeEvent, RuntimeEventEnvelope,
};
