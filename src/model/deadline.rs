//! Shared runtime-owned model request deadline semantics.
//!
//! A model request has one response-start phase followed by one streaming
//! phase. This module owns only the small policy/state vocabulary needed by
//! the primary Agent Loop and the model-backed summarizer. It deliberately
//! does not own retries, publication, cancellation, or durable settlement.

use std::time::Duration;

use crate::model::event::ModelEvent;

/// The finite response-start timeout used when a runtime configuration does
/// not provide another value.
pub const DEFAULT_RESPONSE_START_TIMEOUT: Duration = Duration::from_secs(30);

/// The finite stream-idle timeout used when a runtime configuration does not
/// provide another value.
pub const DEFAULT_STREAM_IDLE_TIMEOUT: Duration = Duration::from_secs(15);

/// Runtime/session execution policy for one admitted model request.
///
/// This is deliberately not part of [`crate::model::ModelRequest`], request
/// snapshots, canonical history, or provider continuation state. An actual
/// request receives a copy at admission and the request-local deadline state
/// owns the phase transitions while that request is live.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelTimeoutPolicy {
    /// Maximum time from request dispatch until the first generation event.
    pub response_start_timeout: Duration,
    /// Maximum time between generation/liveness events after generation has
    /// begun.
    pub stream_idle_timeout: Duration,
}

impl Default for ModelTimeoutPolicy {
    fn default() -> Self {
        Self {
            response_start_timeout: DEFAULT_RESPONSE_START_TIMEOUT,
            stream_idle_timeout: DEFAULT_STREAM_IDLE_TIMEOUT,
        }
    }
}

impl ModelTimeoutPolicy {
    /// Creates one finite model timeout policy.
    #[must_use]
    pub const fn new(response_start_timeout: Duration, stream_idle_timeout: Duration) -> Self {
        Self {
            response_start_timeout,
            stream_idle_timeout,
        }
    }

    /// Whether both configured deadlines can make progress.
    #[must_use]
    pub const fn is_positive(self) -> bool {
        !self.response_start_timeout.is_zero() && !self.stream_idle_timeout.is_zero()
    }
}

/// The semantic liveness class of a normalized model event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelEventProgress {
    /// Lifecycle evidence only; it does not prove provider response progress.
    Lifecycle,
    /// Provider-derived generation progress.
    Generation,
    /// Provider-derived liveness progress that only resets an already
    /// streaming request.
    Liveness,
    /// The provider request has ended.
    Terminal,
}

impl ModelEventProgress {
    /// Classifies a normalized provider event for deadline semantics.
    #[must_use]
    pub const fn classify(event: &ModelEvent) -> Self {
        match event {
            ModelEvent::Started => Self::Lifecycle,
            ModelEvent::TextDelta { .. }
            | ModelEvent::ReasoningDelta { .. }
            | ModelEvent::RefusalDelta { .. }
            | ModelEvent::ToolCallStarted { .. }
            | ModelEvent::ToolCallArgumentsDelta { .. }
            | ModelEvent::ToolCallCompleted { .. } => Self::Generation,
            ModelEvent::UsageUpdate { .. } | ModelEvent::ContinuationState { .. } => Self::Liveness,
            ModelEvent::Completed { .. } | ModelEvent::Failed { .. } => Self::Terminal,
        }
    }
}

/// The request-local phase that owns the current deadline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelDeadlinePhase {
    /// No generation event has been observed yet.
    AwaitingGeneration,
    /// Generation has begun; the deadline is now stream-idle.
    Streaming,
    /// A terminal event transferred deadline ownership away.
    Terminal,
}

/// The narrow request-local deadline state machine shared by model paths.
///
/// The caller supplies the runtime monotonic timestamp at construction and
/// at each event. Waiting on that timestamp is intentionally left to the
/// caller so the Agent Loop and summarizer retain ownership of their own
/// arbitration and outcome semantics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelRequestDeadline {
    policy: ModelTimeoutPolicy,
    phase: ModelDeadlinePhase,
    deadline_millis: Option<u64>,
}

impl ModelRequestDeadline {
    /// Starts the response-start deadline at the dispatch frontier.
    #[must_use]
    pub fn new(policy: ModelTimeoutPolicy, now_millis: u64) -> Self {
        Self {
            policy,
            phase: ModelDeadlinePhase::AwaitingGeneration,
            deadline_millis: Some(Self::deadline_after(
                now_millis,
                policy.response_start_timeout,
            )),
        }
    }

    /// Applies one provider event to the request-local phase machine.
    pub fn observe(&mut self, event: &ModelEvent, now_millis: u64) {
        if self.phase == ModelDeadlinePhase::Terminal {
            return;
        }
        match ModelEventProgress::classify(event) {
            ModelEventProgress::Lifecycle => {}
            ModelEventProgress::Generation => {
                self.phase = ModelDeadlinePhase::Streaming;
                self.deadline_millis = Some(Self::deadline_after(
                    now_millis,
                    self.policy.stream_idle_timeout,
                ));
            }
            ModelEventProgress::Liveness => {
                if self.phase == ModelDeadlinePhase::Streaming {
                    self.deadline_millis = Some(Self::deadline_after(
                        now_millis,
                        self.policy.stream_idle_timeout,
                    ));
                }
            }
            ModelEventProgress::Terminal => {
                self.phase = ModelDeadlinePhase::Terminal;
                self.deadline_millis = None;
            }
        }
    }

    /// The current phase.
    #[must_use]
    pub const fn phase(self) -> ModelDeadlinePhase {
        self.phase
    }

    /// The current absolute deadline, or `None` after terminal settlement.
    #[must_use]
    pub const fn deadline_millis(self) -> Option<u64> {
        self.deadline_millis
    }

    fn deadline_after(now_millis: u64, duration: Duration) -> u64 {
        now_millis.saturating_add(u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
    }
}

#[cfg(test)]
mod tests {
    use super::{ModelDeadlinePhase, ModelEventProgress, ModelRequestDeadline, ModelTimeoutPolicy};
    use crate::message::types::ContentBlockIndex;
    use crate::model::event::ModelEvent;
    use crate::model::types::ModelUsage;
    use crate::runtime::continuation::{OpenAiResponsesContinuation, ProviderContinuationState};
    use crate::runtime::identity::{ToolCallId, ToolId};
    use crate::runtime::monotonic::{ManualMonotonicClock, MonotonicClock};
    use crate::tools::types::ToolCall;

    fn policy() -> ModelTimeoutPolicy {
        ModelTimeoutPolicy::new(
            std::time::Duration::from_millis(10),
            std::time::Duration::from_millis(20),
        )
    }

    fn text() -> ModelEvent {
        ModelEvent::TextDelta {
            block_index: ContentBlockIndex::new(0),
            text: "x".to_owned(),
        }
    }

    #[test]
    fn classification_matches_the_request_contract() {
        assert_eq!(
            ModelEventProgress::classify(&ModelEvent::Started),
            ModelEventProgress::Lifecycle
        );
        assert_eq!(
            ModelEventProgress::classify(&text()),
            ModelEventProgress::Generation
        );
        assert_eq!(
            ModelEventProgress::classify(&ModelEvent::ReasoningDelta {
                block_index: ContentBlockIndex::new(0),
                text: "reasoning".to_owned(),
            }),
            ModelEventProgress::Generation
        );
        assert_eq!(
            ModelEventProgress::classify(&ModelEvent::RefusalDelta {
                block_index: ContentBlockIndex::new(0),
                text: "refusal".to_owned(),
            }),
            ModelEventProgress::Generation
        );
        assert_eq!(
            ModelEventProgress::classify(&ModelEvent::ToolCallStarted {
                block_index: ContentBlockIndex::new(0),
                call: crate::tools::types::ToolCallStart {
                    id: ToolCallId::new("call-1"),
                    tool_id: ToolId::new("tool-1"),
                    name: "tool".to_owned(),
                },
            }),
            ModelEventProgress::Generation
        );
        assert_eq!(
            ModelEventProgress::classify(&ModelEvent::ToolCallArgumentsDelta {
                block_index: ContentBlockIndex::new(0),
                call_id: ToolCallId::new("call-1"),
                arguments_delta: "{}".to_owned(),
            }),
            ModelEventProgress::Generation
        );
        assert_eq!(
            ModelEventProgress::classify(&ModelEvent::ToolCallCompleted {
                block_index: ContentBlockIndex::new(0),
                call: ToolCall {
                    id: ToolCallId::new("call-1"),
                    tool_id: ToolId::new("tool-1"),
                    name: "tool".to_owned(),
                    arguments: serde_json::json!({}),
                },
            }),
            ModelEventProgress::Generation
        );
        assert_eq!(
            ModelEventProgress::classify(&ModelEvent::UsageUpdate {
                usage: ModelUsage {
                    input_tokens: 1,
                    output_tokens: 1,
                    total_tokens: 2,
                    details: None,
                },
            }),
            ModelEventProgress::Liveness
        );
        assert_eq!(
            ModelEventProgress::classify(&ModelEvent::ContinuationState {
                block_index: ContentBlockIndex::new(0),
                state: ProviderContinuationState::OpenAiResponses(
                    OpenAiResponsesContinuation::Stored {
                        previous_response_id: "response-1".to_owned(),
                    },
                ),
            }),
            ModelEventProgress::Liveness
        );
        assert_eq!(
            ModelEventProgress::classify(&ModelEvent::Failed {
                error: crate::model::error::ModelError {
                    kind: crate::model::error::ModelErrorKind::Transport,
                    message: "failed".to_owned(),
                    retry_disposition: crate::model::error::ModelRetryDisposition::Never,
                    retry_after_ms: None,
                    provider_code: None,
                    context_overflow: None,
                },
            }),
            ModelEventProgress::Terminal
        );
    }

    #[test]
    fn lifecycle_and_early_liveness_keep_response_start_deadline() {
        let clock = ManualMonotonicClock::new();
        let mut deadline = ModelRequestDeadline::new(policy(), clock.now_millis());
        assert_eq!(deadline.phase(), ModelDeadlinePhase::AwaitingGeneration);
        assert_eq!(deadline.deadline_millis(), Some(10));
        deadline.observe(&ModelEvent::Started, clock.now_millis());
        clock.advance(5);
        deadline.observe(
            &ModelEvent::UsageUpdate {
                usage: ModelUsage {
                    input_tokens: 1,
                    output_tokens: 1,
                    total_tokens: 2,
                    details: None,
                },
            },
            clock.now_millis(),
        );
        assert_eq!(deadline.phase(), ModelDeadlinePhase::AwaitingGeneration);
        assert_eq!(deadline.deadline_millis(), Some(10));
    }

    #[test]
    fn generation_and_liveness_reset_stream_idle_until_terminal() {
        let clock = ManualMonotonicClock::new();
        let mut deadline = ModelRequestDeadline::new(policy(), clock.now_millis());
        clock.advance(10);
        deadline.observe(&text(), clock.now_millis());
        assert_eq!(deadline.phase(), ModelDeadlinePhase::Streaming);
        assert_eq!(deadline.deadline_millis(), Some(30));
        clock.advance(5);
        deadline.observe(
            &ModelEvent::UsageUpdate {
                usage: ModelUsage {
                    input_tokens: 2,
                    output_tokens: 2,
                    total_tokens: 4,
                    details: None,
                },
            },
            clock.now_millis(),
        );
        assert_eq!(deadline.deadline_millis(), Some(35));
        deadline.observe(
            &ModelEvent::Completed {
                finish_reason: crate::model::finish::ModelFinishReason::Stop,
                usage: None,
            },
            clock.now_millis(),
        );
        assert_eq!(deadline.phase(), ModelDeadlinePhase::Terminal);
        assert_eq!(deadline.deadline_millis(), None);
        clock.advance(100);
        deadline.observe(&text(), clock.now_millis());
        assert_eq!(deadline.phase(), ModelDeadlinePhase::Terminal);
        assert_eq!(deadline.deadline_millis(), None);
    }

    #[test]
    fn policy_is_copied_at_each_request_admission() {
        let clock = ManualMonotonicClock::new();
        let first_policy = policy();
        let first = ModelRequestDeadline::new(first_policy, clock.now_millis());
        let later_policy = ModelTimeoutPolicy::new(
            std::time::Duration::from_millis(100),
            std::time::Duration::from_millis(200),
        );
        clock.advance(10);
        let later = ModelRequestDeadline::new(later_policy, clock.now_millis());

        assert_eq!(first.deadline_millis(), Some(10));
        assert_eq!(later.deadline_millis(), Some(110));
    }
}
