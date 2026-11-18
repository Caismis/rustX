use crate::runtime::identity::{AttemptId, MessageId, RequestId, TurnId};
use serde::{Deserialize, Serialize};

/// The identity of one actual primary request attempt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RequestIdentity {
    /// The attempt that owns this request.
    pub attempt_id: AttemptId,
    /// The logical turn/step within that attempt.
    pub turn: TurnId,
    /// The shared monotonically increasing actual-request ordinal within the
    /// logical model step. Zero is the initial request; transient and
    /// context-overflow recovery requests share the same sequence.
    pub retry_number: u32,
}

impl RequestIdentity {
    /// Derives the stable durable identity of this actual request.
    #[must_use]
    pub fn request_id(&self) -> RequestId {
        RequestId::new(format!(
            "request:{}:{}:{}:{}:{}",
            self.attempt_id.as_str().len(),
            self.attempt_id,
            self.turn.as_str().len(),
            self.turn,
            self.retry_number
        ))
    }

    /// Derives the provisional Assistant message identity frozen for this
    /// request generation.
    ///
    /// The Request Snapshot is the authority for this mapping. Publication
    /// streams and canonical acceptance must use the value frozen here rather
    /// than deriving a second identity from live Agent Loop state.
    #[must_use]
    pub fn provisional_message_id(&self) -> MessageId {
        if self.retry_number == 0 {
            MessageId::new(format!("{}-agent-{}", self.attempt_id, self.turn))
        } else {
            MessageId::new(format!(
                "{}-agent-{}-retry-{}",
                self.attempt_id, self.turn, self.retry_number
            ))
        }
    }
}
