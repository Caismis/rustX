//! Shared runtime-owned model request deadline semantics.
//!
//! A model request has one response-start phase followed by one streaming
//! phase. This module owns only the small policy/state vocabulary needed by
//! the primary Agent Loop and the model-backed summarizer. It deliberately
//! does not own retries, publication, cancellation, or durable settlement.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::model::adapter::{ModelStreamItem, ModelStreamProgress};
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
///
/// The serde representation exists for exactly one internal boundary: the
/// typed `SubagentChildSpec`, through which a launched subagent child
/// inherits its parent runtime's frozen policy (Issue #138).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ModelTimeoutPolicy {
    /// Maximum time from request dispatch until the first generation progress.
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

/// The semantic progress class of one provider stream item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelProgress {
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

impl ModelProgress {
    /// Classifies canonical events and explicit adapter progress for deadline
    /// semantics.
    #[must_use]
    pub const fn classify(item: &ModelStreamItem) -> Self {
        match item {
            ModelStreamItem::Event(event) => match event {
                ModelEvent::Started => Self::Lifecycle,
                ModelEvent::TextDelta { .. }
                | ModelEvent::ReasoningDelta { .. }
                | ModelEvent::RefusalDelta { .. }
                | ModelEvent::ToolCallStarted { .. }
                | ModelEvent::ToolCallArgumentsDelta { .. }
                | ModelEvent::ToolCallCompleted { .. } => Self::Generation,
                ModelEvent::UsageUpdate { .. } | ModelEvent::ContinuationState { .. } => {
                    Self::Liveness
                }
                ModelEvent::Completed { .. } | ModelEvent::Failed { .. } => Self::Terminal,
            },
            ModelStreamItem::Progress(ModelStreamProgress::Generation) => Self::Generation,
            ModelStreamItem::Progress(ModelStreamProgress::Liveness) => Self::Liveness,
        }
    }
}

/// The request-local phase that owns the current deadline.
///
/// This is the *state* of the liveness machine. It has one more inhabitant
/// than [`ModelTimeoutPhase`] on purpose: a request whose stream has already
/// terminated is in a real phase, but it owns no deadline and therefore can
/// never expire. The two types are deliberately not the same type, so an
/// unexpirable state has no representation as an expiry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelDeadlinePhase {
    /// No generation progress has been observed yet.
    AwaitingGeneration,
    /// Generation has begun; the deadline is now stream-idle.
    Streaming,
    /// A terminal event transferred deadline ownership away.
    Terminal,
}

/// The transport-liveness contract a request violated when its deadline
/// expired.
///
/// It is a typed fact rather than a substring of a diagnostic message, so no
/// consumer above the model layer has to read prose to learn which liveness
/// contract was violated.
///
/// It is owned here, by the liveness module, and deliberately **not** by
/// `crate::model::generation`: transport liveness asks whether the provider
/// is still producing at all, which is a different question from whether
/// what it produced is usable. A timeout is not a generation-safety fact and
/// is not represented as one.
///
/// The enum has exactly the phases that can expire. It is unreachable from
/// [`ModelDeadlinePhase::Terminal`] because there is no conversion from the
/// state type to this one: the only source of a [`ModelTimeoutPhase`] is
/// [`ModelRequestDeadline::pending`], which yields one only while a deadline
/// actually exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelTimeoutPhase {
    /// The provider produced no generation progress at all.
    ResponseStart,
    /// The provider stopped producing progress after generation began.
    StreamIdle,
}

impl ModelTimeoutPhase {
    /// The human-readable phase name used in runtime diagnostics. The stable
    /// wire value is the serde representation, not this string.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ResponseStart => "response-start",
            Self::StreamIdle => "stream-idle",
        }
    }
}

/// The narrow request-local deadline state machine shared by model paths.
///
/// The caller supplies the runtime monotonic timestamp at construction and
/// at each stream item. Waiting on that timestamp is intentionally left to the
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

    /// Applies one provider stream item to the request-local phase machine.
    pub fn observe(&mut self, item: &ModelStreamItem, now_millis: u64) {
        if self.phase == ModelDeadlinePhase::Terminal {
            return;
        }
        match ModelProgress::classify(item) {
            ModelProgress::Lifecycle => {}
            ModelProgress::Generation => {
                self.phase = ModelDeadlinePhase::Streaming;
                self.deadline_millis = Some(Self::deadline_after(
                    now_millis,
                    self.policy.stream_idle_timeout,
                ));
            }
            ModelProgress::Liveness => {
                if self.phase == ModelDeadlinePhase::Streaming {
                    self.deadline_millis = Some(Self::deadline_after(
                        now_millis,
                        self.policy.stream_idle_timeout,
                    ));
                }
            }
            ModelProgress::Terminal => {
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

    /// The deadline this request is currently running against: the liveness
    /// contract that will be violated, and the absolute monotonic instant at
    /// which it is violated.
    ///
    /// This is the only way to obtain a [`ModelTimeoutPhase`], and it is the
    /// structural reason an impossible expiry cannot be fabricated. A
    /// terminal request owns no deadline, so it yields `None` and there is no
    /// pair for a caller to turn into a timeout; a caller that has a pair
    /// has, by construction, a phase that could really expire.
    #[must_use]
    pub const fn pending(self) -> Option<(ModelTimeoutPhase, u64)> {
        match (self.phase, self.deadline_millis) {
            (ModelDeadlinePhase::AwaitingGeneration, Some(deadline)) => {
                Some((ModelTimeoutPhase::ResponseStart, deadline))
            }
            (ModelDeadlinePhase::Streaming, Some(deadline)) => {
                Some((ModelTimeoutPhase::StreamIdle, deadline))
            }
            _ => None,
        }
    }

    fn deadline_after(now_millis: u64, duration: Duration) -> u64 {
        now_millis.saturating_add(u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ModelDeadlinePhase, ModelProgress, ModelRequestDeadline, ModelTimeoutPhase,
        ModelTimeoutPolicy,
    };
    use crate::message::types::ContentBlockIndex;
    use crate::model::adapter::{ModelStreamItem, ModelStreamProgress};
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

    /// One terminal failure event. Its error payload is irrelevant to
    /// progress classification, so it is built once here rather than spelled
    /// out inside the matrix below.
    fn failed() -> ModelEvent {
        ModelEvent::Failed {
            error: crate::model::error::ModelError::timeout(ModelTimeoutPhase::StreamIdle),
        }
    }

    #[test]
    fn classification_matches_the_request_contract() {
        assert_eq!(
            ModelProgress::classify(&ModelStreamItem::Event(ModelEvent::Started)),
            ModelProgress::Lifecycle
        );
        assert_eq!(
            ModelProgress::classify(&ModelStreamItem::Event(text())),
            ModelProgress::Generation
        );
        assert_eq!(
            ModelProgress::classify(&ModelStreamItem::Event(ModelEvent::ReasoningDelta {
                block_index: ContentBlockIndex::new(0),
                text: "reasoning".to_owned(),
            })),
            ModelProgress::Generation
        );
        assert_eq!(
            ModelProgress::classify(&ModelStreamItem::Event(ModelEvent::RefusalDelta {
                block_index: ContentBlockIndex::new(0),
                text: "refusal".to_owned(),
            })),
            ModelProgress::Generation
        );
        assert_eq!(
            ModelProgress::classify(&ModelStreamItem::Event(ModelEvent::ToolCallStarted {
                block_index: ContentBlockIndex::new(0),
                call: crate::tools::types::ToolCallStart {
                    id: ToolCallId::new("call-1"),
                    tool_id: ToolId::new("tool-1"),
                    name: "tool".to_owned(),
                },
            })),
            ModelProgress::Generation
        );
        assert_eq!(
            ModelProgress::classify(&ModelStreamItem::Event(
                ModelEvent::ToolCallArgumentsDelta {
                    block_index: ContentBlockIndex::new(0),
                    call_id: ToolCallId::new("call-1"),
                    arguments_delta: "{}".to_owned(),
                }
            )),
            ModelProgress::Generation
        );
        assert_eq!(
            ModelProgress::classify(&ModelStreamItem::Event(ModelEvent::ToolCallCompleted {
                block_index: ContentBlockIndex::new(0),
                call: ToolCall {
                    id: ToolCallId::new("call-1"),
                    tool_id: ToolId::new("tool-1"),
                    name: "tool".to_owned(),
                    arguments: serde_json::json!({}),
                },
            })),
            ModelProgress::Generation
        );
        assert_eq!(
            ModelProgress::classify(&ModelStreamItem::Event(ModelEvent::UsageUpdate {
                usage: ModelUsage {
                    input_tokens: 1,
                    output_tokens: 1,
                    total_tokens: 2,
                    details: None,
                },
            })),
            ModelProgress::Liveness
        );
        assert_eq!(
            ModelProgress::classify(&ModelStreamItem::Event(ModelEvent::ContinuationState {
                block_index: ContentBlockIndex::new(0),
                state: ProviderContinuationState::OpenAiResponses(
                    OpenAiResponsesContinuation::Stored {
                        previous_response_id: "response-1".to_owned(),
                    },
                ),
            })),
            ModelProgress::Liveness
        );
        assert_eq!(
            ModelProgress::classify(&ModelStreamItem::Event(failed())),
            ModelProgress::Terminal
        );
        assert_eq!(
            ModelProgress::classify(&ModelStreamItem::Progress(ModelStreamProgress::Generation)),
            ModelProgress::Generation
        );
        assert_eq!(
            ModelProgress::classify(&ModelStreamItem::Progress(ModelStreamProgress::Liveness)),
            ModelProgress::Liveness
        );
    }

    #[test]
    fn lifecycle_and_early_liveness_keep_response_start_deadline() {
        let clock = ManualMonotonicClock::new();
        let mut deadline = ModelRequestDeadline::new(policy(), clock.now_millis());
        assert_eq!(deadline.phase(), ModelDeadlinePhase::AwaitingGeneration);
        assert_eq!(deadline.deadline_millis(), Some(10));
        deadline.observe(
            &ModelStreamItem::Event(ModelEvent::Started),
            clock.now_millis(),
        );
        clock.advance(5);
        deadline.observe(
            &ModelStreamItem::Event(ModelEvent::UsageUpdate {
                usage: ModelUsage {
                    input_tokens: 1,
                    output_tokens: 1,
                    total_tokens: 2,
                    details: None,
                },
            }),
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
        deadline.observe(&ModelStreamItem::Event(text()), clock.now_millis());
        assert_eq!(deadline.phase(), ModelDeadlinePhase::Streaming);
        assert_eq!(deadline.deadline_millis(), Some(30));
        clock.advance(5);
        deadline.observe(
            &ModelStreamItem::Event(ModelEvent::UsageUpdate {
                usage: ModelUsage {
                    input_tokens: 2,
                    output_tokens: 2,
                    total_tokens: 4,
                    details: None,
                },
            }),
            clock.now_millis(),
        );
        assert_eq!(deadline.deadline_millis(), Some(35));
        deadline.observe(
            &ModelStreamItem::Event(ModelEvent::Completed {
                finish_reason: crate::model::finish::ModelFinishReason::Stop,
                usage: None,
            }),
            clock.now_millis(),
        );
        assert_eq!(deadline.phase(), ModelDeadlinePhase::Terminal);
        assert_eq!(deadline.deadline_millis(), None);
        clock.advance(100);
        deadline.observe(&ModelStreamItem::Event(text()), clock.now_millis());
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

    #[test]
    fn explicit_progress_has_the_same_phase_semantics_as_canonical_output() {
        let clock = ManualMonotonicClock::new();
        let mut deadline = ModelRequestDeadline::new(policy(), clock.now_millis());
        clock.advance(10);
        deadline.observe(
            &ModelStreamItem::Progress(ModelStreamProgress::Generation),
            clock.now_millis(),
        );
        assert_eq!(deadline.phase(), ModelDeadlinePhase::Streaming);
        assert_eq!(deadline.deadline_millis(), Some(30));
        clock.advance(5);
        deadline.observe(
            &ModelStreamItem::Progress(ModelStreamProgress::Liveness),
            clock.now_millis(),
        );
        assert_eq!(deadline.deadline_millis(), Some(35));
    }
}
