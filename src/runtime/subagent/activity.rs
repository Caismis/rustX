//! The subagent live-activity observation plane (Issue #178).
//!
//! A subagent child has exactly one lifecycle authority (the parent-side
//! [`SubagentRegistry`](super::SubagentRegistry)), exactly one durable
//! execution history (the child's own conversation), and any number of
//! **disposable observation projections**. This module is the vocabulary of
//! those projections:
//!
//! ```text
//! child runtime semantic observations
//!         |  fold (SubagentObservationProjector, child side)
//!         v
//! SubagentObservation   latest-value, revision-stamped
//!         |  dispatcher activity-slot coalescing -> Activity IPC frame
//!         v
//! parent registry read model (SubagentSnapshot.observation)
//!         |  existing push-only snapshot seam
//!         v
//! Runtime Client projection / TUI
//! ```
//!
//! # Zero semantic authority
//!
//! An observation is a read model of what the child is doing *right now*.
//! It is never durable, never enters the parent's Event Journal, never
//! enters any model context, and never carries result content: the
//! successful terminal answer exists only in the durable terminal inbound
//! publication. Nothing in this module can change a lifecycle decision,
//! and no consumer may treat an activity as evidence of progress owed —
//! lifecycle states remain the only truth about whether a child is alive,
//! settling, or settled.
//!
//! # The neutral state
//!
//! [`SubagentActivity::AwaitingActivity`] is the one neutral state: the
//! projection rests there between objective transitions and every terminal
//! settlement resets to it. A distinct `Quiescent` state was considered and
//! rejected: no objective child-granularity signal exists that is distinct
//! from "no known objective transition is in flight", so a second neutral
//! state could only ever be a relabelled guess.
//!
//! # Coalescing
//!
//! High-frequency transitions (tool progress in particular) are coalesced
//! with latest-value semantics: the child publishes its projection into the
//! dispatcher's disposable latest-value activity slot, so a slow consumer
//! observes the newest revision and provably never blocks child execution
//! on observation delivery. The monotonically increasing `revision` lets
//! the parent drop stale or reordered updates.
//!
//! # Timestamps
//!
//! [`SubagentObservation::last_activity_at`] is stamped by the child at the
//! instant the transition is folded: the `ConversationObservation::Event`
//! lane deliberately carries the bare [`RuntimeEvent`] without its durable
//! envelope, so no envelope timestamp is available at the fold point. The
//! fold runs immediately after the source fact committed, so the stamp is
//! the live observation time, never a backdated durable time.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::events::types::RuntimeEvent;
use crate::model::catalog::ReasoningProfileId;
use crate::model::frozen::FrozenModelSpec;
use crate::runtime::identity::{RequestId, ToolCallId, ToolId};
use crate::runtime::interaction::InteractionKind;
use crate::runtime::observation::ConversationObservation;
use crate::tools::types::ToolProgress;

/// The latest live-activity projection of one subagent child.
///
/// Owned and advanced by the child; the parent registry stores the newest
/// revision it has seen. Strictly increasing `revision` per applied
/// transition makes stale or reordered deliveries detectable and droppable.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubagentObservation {
    /// The child-owned projection revision; strictly increasing per applied
    /// transition.
    pub revision: u64,
    /// What the child is observably doing right now.
    pub activity: SubagentActivity,
    /// When the latest applied transition was folded (child-side clock).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_activity_at: Option<DateTime<Utc>>,
    /// Cumulative transition counters since the child started.
    pub counters: SubagentActivityCounters,
}

impl Default for SubagentObservation {
    /// The initial, neutral projection of a child that has not yet reported
    /// any activity (revision 0).
    fn default() -> Self {
        Self {
            revision: 0,
            activity: SubagentActivity::AwaitingActivity,
            last_activity_at: None,
            counters: SubagentActivityCounters::default(),
        }
    }
}

impl SubagentObservation {
    /// Resets the projection to the terminal-neutral state at settlement
    /// (parent registry side): the lifecycle is the terminal truth, and
    /// live activity is a live-only signal, so a settled child never
    /// projects a stale in-flight activity. Counters and the last-activity
    /// timestamp are kept as the final record of what the child did.
    pub(crate) fn settle_neutral(&mut self) {
        self.activity = SubagentActivity::AwaitingActivity;
        self.revision += 1;
    }
}

/// The child-side projector: owns the wire projection plus the fold-local
/// state that must not cross the wire.
///
/// The projection is rebuilt by folding every child runtime observation;
/// some folds need state that is not part of the projection itself — the
/// pending retry ordinal of the next model request, and the set of
/// concurrently active tool executions — and that state must never be
/// serialized into [`SubagentObservation`], so it lives here.
///
/// # Parallel tool executions
///
/// A parallel tool group (`ToolConcurrencyPolicy::Parallel`) can have more
/// than one foreground tool executing at once. The projector tracks every
/// active call internally, but the wire activity stays bounded to one
/// deterministic representative:
///
/// - `ToolExecutionStarted(call)` registers the call and makes it visible:
///   the most recently started call is the representative.
/// - A progress report (live or durable) updates its own call only and
///   makes that call visible: the call that most recently produced an
///   objective activity fact is the representative. Progress of a settled
///   or unknown call is ignored — a stale report can never resurrect a
///   completed execution.
/// - `ToolExecutionCompleted`/`Failed(call)` removes exactly that call and
///   counts it exactly once. If the settled call was visible, the
///   representative falls back to the latest-started surviving call; the
///   projection only returns to neutral once NO active call remains.
#[derive(Debug, Default)]
pub(crate) struct SubagentObservationProjector {
    /// The latest folded wire projection.
    observation: SubagentObservation,
    /// Retry ordinal carried by the NEXT `ModelRequestStarted`: set by
    /// `ModelRetryScheduled`, consumed (reset to 0) by the next request
    /// start. This is the current request's retry ordinal, kept strictly
    /// separate from the cumulative counter — a fresh request of a later
    /// turn never inherits an earlier retry's ordinal.
    pending_retry: u32,
    /// Every currently executing tool call, keyed by call id. Fold-local
    /// only: never serialized into the wire projection.
    active_tools: BTreeMap<ToolCallId, ActiveToolProjection>,
    /// Monotonic start ordinal used to pick a deterministic surviving
    /// representative (latest-started) — never hash-map iteration order.
    next_tool_order: u64,
    /// The call currently exposed as the bounded wire representative.
    /// Invariant: `Some(call)` exactly while the activity is
    /// `SubagentActivity::Tool { tool_call_id: call, .. }` and `call` is in
    /// `active_tools`.
    visible_tool: Option<ToolCallId>,
}

/// Fold-local projection of one active tool execution (never serialized).
#[derive(Debug, Clone)]
struct ActiveToolProjection {
    /// The executed tool.
    tool_id: ToolId,
    /// The latest bounded progress notification this call reported.
    progress: Option<ToolProgress>,
    /// The deterministic start ordinal of this call.
    started_order: u64,
}

impl SubagentObservationProjector {
    /// Folds one child runtime observation into the projection.
    ///
    /// Returns whether a transition was applied — i.e. whether the revision
    /// advanced and the projection is worth forwarding. Observations that
    /// carry no activity signal (message commits, status emissions, journal
    /// bookkeeping) leave the projection untouched: they bump no revision
    /// and stamp no timestamp.
    pub(crate) fn fold(
        &mut self,
        observation: &ConversationObservation,
        now: DateTime<Utc>,
    ) -> bool {
        let applied = match observation {
            ConversationObservation::Event { event, .. } => self.fold_event(event),
            // One live (not yet durable) foreground tool progress report
            // (Issue #178): exactly the durable `ToolExecutionProgress`
            // semantics — applies only to the current in-flight `Tool`
            // activity with the matching call id; otherwise ignored.
            ConversationObservation::ToolProgress {
                tool_call_id,
                progress,
                ..
            } => self.apply_tool_progress(tool_call_id, progress),
            ConversationObservation::InteractionPending { request, .. } => {
                let on = match &request.kind {
                    InteractionKind::Approval { tool_id, .. } => SubagentWaitReason::Approval {
                        tool_id: tool_id.clone(),
                    },
                    InteractionKind::Questionnaire { .. } => SubagentWaitReason::Questionnaire,
                };
                self.visible_tool = None;
                self.observation.activity = SubagentActivity::Waiting { on };
                true
            }
            // The wait is objectively over. If a sibling tool is still
            // running (a parallel group approved while other calls execute),
            // the projection returns to a deterministic surviving tool
            // representative; only with no active call left does it go
            // neutral.
            ConversationObservation::InteractionSettled { .. } => {
                self.project_tool_survivor_or_neutral();
                true
            }
            _ => false,
        };
        if applied {
            self.observation.revision += 1;
            self.observation.last_activity_at = Some(now);
        }
        applied
    }

    /// The latest folded wire projection.
    pub(crate) fn observation(&self) -> &SubagentObservation {
        &self.observation
    }

    /// Folds one canonical runtime event of the child's attempt.
    fn fold_event(&mut self, event: &RuntimeEvent) -> bool {
        match event {
            RuntimeEvent::ModelRequestStarted { request_id, .. } => {
                // The retry ordinal of THIS request: whatever retry the
                // scheduler armed, consumed exactly once. A later turn's
                // fresh request therefore projects retry 0 regardless of
                // how many retries earlier turns accumulated.
                let retry = std::mem::take(&mut self.pending_retry);
                self.observation.counters.model_requests += 1;
                self.visible_tool = None;
                self.observation.activity = SubagentActivity::Model {
                    request_id: request_id.clone(),
                    retry,
                };
                true
            }
            RuntimeEvent::ModelRequestCompleted { .. }
            | RuntimeEvent::ModelRequestFailed { .. }
            | RuntimeEvent::CompactionCompleted { .. }
            | RuntimeEvent::CompactionFailed { .. } => {
                self.visible_tool = None;
                self.observation.activity = SubagentActivity::AwaitingActivity;
                true
            }
            RuntimeEvent::ModelRetryScheduled { retry_number, .. } => {
                self.pending_retry = *retry_number;
                self.observation.counters.model_retries += 1;
                self.visible_tool = None;
                self.observation.activity = SubagentActivity::RetryingModel {
                    retry: *retry_number,
                };
                true
            }
            RuntimeEvent::ToolExecutionStarted {
                tool_call_id,
                tool_id,
            } => {
                self.next_tool_order += 1;
                self.active_tools.insert(
                    tool_call_id.clone(),
                    ActiveToolProjection {
                        tool_id: tool_id.clone(),
                        progress: None,
                        started_order: self.next_tool_order,
                    },
                );
                self.visible_tool = Some(tool_call_id.clone());
                self.observation.activity = SubagentActivity::Tool {
                    tool_call_id: tool_call_id.clone(),
                    tool_id: tool_id.clone(),
                    progress: None,
                };
                true
            }
            RuntimeEvent::ToolExecutionProgress {
                tool_call_id,
                progress,
                ..
            } => self.apply_tool_progress(tool_call_id, progress),
            RuntimeEvent::ToolExecutionCompleted { tool_call_id, .. }
            | RuntimeEvent::ToolExecutionFailed { tool_call_id, .. } => {
                self.settle_tool_execution(tool_call_id)
            }
            RuntimeEvent::CompactionStarted => {
                self.visible_tool = None;
                self.observation.activity = SubagentActivity::Compacting;
                true
            }
            _ => false,
        }
    }

    /// Applies one tool progress report to the projection.
    ///
    /// Progress belongs to exactly one in-flight execution: it updates its
    /// own call's fold-local state and makes that call the visible
    /// representative — the call that most recently produced an objective
    /// activity fact is what the bounded wire projection shows. A report
    /// for a settled or unknown call is stale and can never resurrect a
    /// completed execution.
    fn apply_tool_progress(&mut self, tool_call_id: &ToolCallId, progress: &ToolProgress) -> bool {
        let tool_id = {
            let Some(tool) = self.active_tools.get_mut(tool_call_id) else {
                return false;
            };
            tool.progress = Some(progress.clone());
            tool.tool_id.clone()
        };
        self.visible_tool = Some(tool_call_id.clone());
        self.observation.activity = SubagentActivity::Tool {
            tool_call_id: tool_call_id.clone(),
            tool_id,
            progress: Some(progress.clone()),
        };
        true
    }

    /// Settles exactly one tool execution.
    ///
    /// Removes only that call from the active set and counts it exactly
    /// once; a settlement of a call that is not active (duplicate or
    /// never-started-here) carries no signal. If the settled call was the
    /// visible representative, the projection falls back to the
    /// latest-started surviving call — sibling completion never resets the
    /// projection to neutral while another call remains active.
    fn settle_tool_execution(&mut self, tool_call_id: &ToolCallId) -> bool {
        if self.active_tools.remove(tool_call_id).is_none() {
            return false;
        }
        self.observation.counters.tool_executions += 1;
        if self.visible_tool.as_ref() == Some(tool_call_id) {
            self.project_tool_survivor_or_neutral();
        }
        true
    }

    /// Re-projects the deterministic surviving tool representative — the
    /// latest-started call still active — or returns the projection to the
    /// neutral state once no active call remains.
    fn project_tool_survivor_or_neutral(&mut self) {
        let survivor = self
            .active_tools
            .iter()
            .max_by_key(|(_, tool)| tool.started_order)
            .map(|(tool_call_id, tool)| (tool_call_id.clone(), tool.clone()));
        if let Some((tool_call_id, tool)) = survivor {
            self.visible_tool = Some(tool_call_id.clone());
            self.observation.activity = SubagentActivity::Tool {
                tool_call_id,
                tool_id: tool.tool_id,
                progress: tool.progress,
            };
        } else {
            self.visible_tool = None;
            self.observation.activity = SubagentActivity::AwaitingActivity;
        }
    }
}

/// Cumulative activity counters of one child, kept across activity changes
/// so a consumer can observe throughput without folding every transition.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubagentActivityCounters {
    /// Model requests started.
    pub model_requests: u64,
    /// Model retry schedules observed (cumulative count).
    pub model_retries: u64,
    /// Tool executions finished (completed plus failed).
    pub tool_executions: u64,
}

/// What the child is observably doing right now.
///
/// Every variant derives from an objective child runtime transition; there
/// is deliberately no idle/quiescent variant beyond the neutral
/// `AwaitingActivity` (see the module documentation).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum SubagentActivity {
    /// No objective transition is in flight: the child is between a model
    /// request and its tool work, waiting on the provider response of an
    /// already-started request's successor, or settled. This is also the
    /// terminal-neutral state every settlement resets to.
    #[default]
    AwaitingActivity,
    /// A model request is in flight.
    Model {
        /// The in-flight provider request.
        request_id: RequestId,
        /// The retry ordinal of THIS request: 0 for a first attempt, `n`
        /// for the request the nth scheduled retry armed.
        retry: u32,
    },
    /// The last model request failed and a retry is scheduled.
    RetryingModel {
        /// The scheduled retry ordinal.
        retry: u32,
    },
    /// A tool execution is in flight.
    Tool {
        /// The executing tool call.
        tool_call_id: ToolCallId,
        /// The executed tool.
        tool_id: ToolId,
        /// The latest bounded progress notification, when the tool
        /// reported any.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        progress: Option<ToolProgress>,
    },
    /// A context compaction is in flight.
    Compacting,
    /// The child is blocked on a native interaction.
    Waiting {
        /// What the child waits on.
        on: SubagentWaitReason,
    },
}

/// Why the child is blocked on a native interaction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum SubagentWaitReason {
    /// A tool invocation awaits an approval decision.
    Approval {
        /// The tool awaiting approval.
        tool_id: ToolId,
    },
    /// A questionnaire awaits an answer.
    Questionnaire,
}

/// The safe, redacted execution profile of one child, frozen at child start
/// (Issue #178).
///
/// Derived from the frozen model authority exactly once by
/// [`SubagentExecutionProfile::from_frozen`]: it carries only the effective
/// model identity and reasoning selection. Credentials, endpoints, provider
/// bindings, and every other binding internal are never projected.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct SubagentExecutionProfile {
    /// The effective fully qualified model reference (`provider/model`).
    pub model: String,
    /// The selected reasoning profile, when the model declares any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_profile: Option<ReasoningProfileId>,
    /// Whether the selected profile semantically enables reasoning.
    pub reasoning_enabled: bool,
}

impl SubagentExecutionProfile {
    /// Derives the redacted profile from the child's frozen model authority.
    #[must_use]
    pub fn from_frozen(frozen: &FrozenModelSpec) -> Self {
        Self {
            model: frozen.primary.model.to_string(),
            reasoning_profile: frozen.primary.reasoning_profile.clone(),
            reasoning_enabled: frozen.primary.reasoning_enabled,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::identity::AttemptId;

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-09-02T10:00:00Z")
            .expect("timestamp")
            .with_timezone(&Utc)
    }

    fn model_error() -> crate::model::ModelError {
        crate::model::ModelError {
            kind: crate::model::error::ModelErrorKind::Timeout,
            message: "timed out".to_owned(),
            retry_disposition: crate::model::error::ModelRetryDisposition::Transient,
            retry_after_ms: None,
            provider_code: None,
            context_overflow: None,
        }
    }

    fn tool_result() -> crate::tools::types::ToolExecutionResult {
        crate::tools::types::ToolExecutionResult {
            status: crate::tools::types::ToolExecutionStatus::Success,
            content: Vec::new(),
            duration_ms: 1,
            exit_code: None,
            artifacts: Vec::new(),
            truncation: None,
            managed_output: None,
        }
    }

    fn event(event: RuntimeEvent) -> ConversationObservation {
        ConversationObservation::Event {
            attempt_id: AttemptId::new("attempt-1"),
            event,
        }
    }

    fn assert_folded(
        projector: &mut SubagentObservationProjector,
        observation: &ConversationObservation,
    ) {
        let revision = projector.observation().revision;
        assert!(
            projector.fold(observation, now()),
            "the observation carries an activity signal"
        );
        assert_eq!(projector.observation().revision, revision + 1);
        assert_eq!(projector.observation().last_activity_at, Some(now()));
    }

    fn assert_ignored(
        projector: &mut SubagentObservationProjector,
        observation: &ConversationObservation,
    ) {
        let before = projector.observation().clone();
        assert!(!projector.fold(observation, now()));
        assert_eq!(
            *projector.observation(),
            before,
            "an irrelevant observation is a no-op"
        );
    }

    #[test]
    fn model_request_start_and_complete_cycle() {
        let mut projector = SubagentObservationProjector::default();
        assert_eq!(projector.observation.revision, 0);
        assert_eq!(
            projector.observation.activity,
            SubagentActivity::AwaitingActivity
        );
        assert_eq!(projector.observation.last_activity_at, None);

        assert_folded(
            &mut projector,
            &event(RuntimeEvent::ModelRequestStarted {
                request_id: RequestId::new("req-1"),
                model: "local/model".to_owned(),
            }),
        );
        assert_eq!(
            projector.observation.activity,
            SubagentActivity::Model {
                request_id: RequestId::new("req-1"),
                retry: 0,
            }
        );
        assert_eq!(projector.observation.counters.model_requests, 1);

        assert_folded(
            &mut projector,
            &event(RuntimeEvent::ModelRequestCompleted {
                request_id: RequestId::new("req-1"),
                finish_reason: crate::model::ModelFinishReason::Stop,
                usage: None,
            }),
        );
        assert_eq!(
            projector.observation.activity,
            SubagentActivity::AwaitingActivity
        );
    }

    #[test]
    fn a_scheduled_retry_marks_the_next_request_as_a_retry() {
        let mut projector = SubagentObservationProjector::default();
        assert_folded(
            &mut projector,
            &event(RuntimeEvent::ModelRequestStarted {
                request_id: RequestId::new("req-1"),
                model: "local/model".to_owned(),
            }),
        );
        assert_folded(
            &mut projector,
            &event(RuntimeEvent::ModelRequestFailed {
                request_id: RequestId::new("req-1"),
                error: model_error(),
                usage: None,
            }),
        );
        assert_eq!(
            projector.observation.activity,
            SubagentActivity::AwaitingActivity
        );

        assert_folded(
            &mut projector,
            &event(RuntimeEvent::ModelRetryScheduled {
                failed_request_id: RequestId::new("req-1"),
                retry_number: 1,
                retry_delay_ms: Some(250),
            }),
        );
        assert_eq!(
            projector.observation.activity,
            SubagentActivity::RetryingModel { retry: 1 }
        );
        assert_eq!(projector.observation.counters.model_retries, 1);

        assert_folded(
            &mut projector,
            &event(RuntimeEvent::ModelRequestStarted {
                request_id: RequestId::new("req-2"),
                model: "local/model".to_owned(),
            }),
        );
        assert_eq!(
            projector.observation.activity,
            SubagentActivity::Model {
                request_id: RequestId::new("req-2"),
                retry: 1,
            }
        );
        assert_eq!(projector.observation.counters.model_requests, 2);
    }

    /// Regression (Issue #178, blocker 7): the retry ordinal is a property
    /// of the CURRENT request, consumed by the next request start — never
    /// of the cumulative retry counter. A retried request projects
    /// `retry: 1`, and a fresh request of a later turn projects `retry: 0`
    /// even though the counter stays cumulative.
    #[test]
    fn a_fresh_request_of_a_later_turn_never_inherits_an_earlier_retry_ordinal() {
        let mut projector = SubagentObservationProjector::default();

        // Turn 1: the request fails, retry #1 is scheduled, and the
        // retried request starts and completes.
        assert_folded(
            &mut projector,
            &event(RuntimeEvent::ModelRequestStarted {
                request_id: RequestId::new("req-1"),
                model: "local/model".to_owned(),
            }),
        );
        assert_folded(
            &mut projector,
            &event(RuntimeEvent::ModelRequestFailed {
                request_id: RequestId::new("req-1"),
                error: model_error(),
                usage: None,
            }),
        );
        assert_folded(
            &mut projector,
            &event(RuntimeEvent::ModelRetryScheduled {
                failed_request_id: RequestId::new("req-1"),
                retry_number: 1,
                retry_delay_ms: Some(250),
            }),
        );
        assert_folded(
            &mut projector,
            &event(RuntimeEvent::ModelRequestStarted {
                request_id: RequestId::new("req-2"),
                model: "local/model".to_owned(),
            }),
        );
        assert_eq!(
            projector.observation.activity,
            SubagentActivity::Model {
                request_id: RequestId::new("req-2"),
                retry: 1,
            },
            "the retried request carries the armed ordinal"
        );
        assert_folded(
            &mut projector,
            &event(RuntimeEvent::ModelRequestCompleted {
                request_id: RequestId::new("req-2"),
                finish_reason: crate::model::ModelFinishReason::Stop,
                usage: None,
            }),
        );

        // Turn 2: a fresh request. The cumulative counter still records one
        // retry, but the ordinal was consumed by req-2 — nothing is armed.
        assert_folded(
            &mut projector,
            &event(RuntimeEvent::ModelRequestStarted {
                request_id: RequestId::new("req-3"),
                model: "local/model".to_owned(),
            }),
        );
        assert_eq!(
            projector.observation.activity,
            SubagentActivity::Model {
                request_id: RequestId::new("req-3"),
                retry: 0,
            },
            "a fresh request of a later turn starts at ordinal 0"
        );
        assert_eq!(
            projector.observation.counters.model_retries, 1,
            "the counter is genuinely cumulative"
        );
        assert_eq!(projector.observation.counters.model_requests, 3);
    }

    /// The live (not yet durable) `ToolProgress` observation (Issue #178)
    /// folds with exactly the durable `ToolExecutionProgress` semantics: it
    /// applies to the current in-flight `Tool` activity with the matching
    /// call id, while the tool still executes, and is ignored for any other
    /// call or when no tool is current.
    #[test]
    fn live_tool_progress_folds_into_the_current_execution() {
        let mut projector = SubagentObservationProjector::default();
        assert_folded(
            &mut projector,
            &event(RuntimeEvent::ToolExecutionStarted {
                tool_call_id: ToolCallId::new("call-1"),
                tool_id: ToolId::new("tool-bash"),
            }),
        );

        // A live report of another call id is ignored.
        assert_ignored(
            &mut projector,
            &ConversationObservation::ToolProgress {
                attempt_id: AttemptId::new("attempt-1"),
                tool_call_id: ToolCallId::new("call-other"),
                tool_id: ToolId::new("tool-bash"),
                progress: ToolProgress {
                    message: Some("stray".to_owned()),
                    ..ToolProgress::default()
                },
            },
        );

        // A live report of the in-flight call projects WHILE the tool still
        // executes — no durable progress fact has committed.
        assert_folded(
            &mut projector,
            &ConversationObservation::ToolProgress {
                attempt_id: AttemptId::new("attempt-1"),
                tool_call_id: ToolCallId::new("call-1"),
                tool_id: ToolId::new("tool-bash"),
                progress: ToolProgress {
                    message: Some("live".to_owned()),
                    completed: Some(1.0),
                    total: Some(4.0),
                },
            },
        );
        assert_eq!(
            projector.observation.activity,
            SubagentActivity::Tool {
                tool_call_id: ToolCallId::new("call-1"),
                tool_id: ToolId::new("tool-bash"),
                progress: Some(ToolProgress {
                    message: Some("live".to_owned()),
                    completed: Some(1.0),
                    total: Some(4.0),
                }),
            }
        );

        // With no tool in flight, a live report is ignored.
        assert_folded(
            &mut projector,
            &event(RuntimeEvent::ToolExecutionCompleted {
                tool_call_id: ToolCallId::new("call-1"),
                tool_id: ToolId::new("tool-bash"),
                result: tool_result(),
            }),
        );
        assert_ignored(
            &mut projector,
            &ConversationObservation::ToolProgress {
                attempt_id: AttemptId::new("attempt-1"),
                tool_call_id: ToolCallId::new("call-1"),
                tool_id: ToolId::new("tool-bash"),
                progress: ToolProgress::default(),
            },
        );
    }

    #[test]
    fn tool_progress_applies_only_to_the_current_execution() {
        let mut projector = SubagentObservationProjector::default();
        assert_folded(
            &mut projector,
            &event(RuntimeEvent::ToolExecutionStarted {
                tool_call_id: ToolCallId::new("call-1"),
                tool_id: ToolId::new("tool-bash"),
            }),
        );
        assert_eq!(
            projector.observation.activity,
            SubagentActivity::Tool {
                tool_call_id: ToolCallId::new("call-1"),
                tool_id: ToolId::new("tool-bash"),
                progress: None,
            }
        );

        // Progress of another call id is ignored: no transition, no
        // revision bump.
        assert_ignored(
            &mut projector,
            &event(RuntimeEvent::ToolExecutionProgress {
                tool_call_id: ToolCallId::new("call-other"),
                tool_id: ToolId::new("tool-bash"),
                execution_id: None,
                progress: ToolProgress {
                    message: Some("stray".to_owned()),
                    ..ToolProgress::default()
                },
            }),
        );

        assert_folded(
            &mut projector,
            &event(RuntimeEvent::ToolExecutionProgress {
                tool_call_id: ToolCallId::new("call-1"),
                tool_id: ToolId::new("tool-bash"),
                execution_id: None,
                progress: ToolProgress {
                    message: Some("halfway".to_owned()),
                    completed: Some(1.0),
                    total: Some(2.0),
                },
            }),
        );
        assert_eq!(
            projector.observation.activity,
            SubagentActivity::Tool {
                tool_call_id: ToolCallId::new("call-1"),
                tool_id: ToolId::new("tool-bash"),
                progress: Some(ToolProgress {
                    message: Some("halfway".to_owned()),
                    completed: Some(1.0),
                    total: Some(2.0),
                }),
            }
        );

        assert_folded(
            &mut projector,
            &event(RuntimeEvent::ToolExecutionCompleted {
                tool_call_id: ToolCallId::new("call-1"),
                tool_id: ToolId::new("tool-bash"),
                result: tool_result(),
            }),
        );
        assert_eq!(
            projector.observation.activity,
            SubagentActivity::AwaitingActivity
        );
        assert_eq!(projector.observation.counters.tool_executions, 1);

        // Progress with no tool in flight is ignored.
        assert_ignored(
            &mut projector,
            &event(RuntimeEvent::ToolExecutionProgress {
                tool_call_id: ToolCallId::new("call-1"),
                tool_id: ToolId::new("tool-bash"),
                execution_id: None,
                progress: ToolProgress::default(),
            }),
        );
    }

    #[test]
    fn failed_tool_executions_count_and_reset() {
        let mut projector = SubagentObservationProjector::default();
        assert_folded(
            &mut projector,
            &event(RuntimeEvent::ToolExecutionStarted {
                tool_call_id: ToolCallId::new("call-1"),
                tool_id: ToolId::new("tool-read"),
            }),
        );
        assert_folded(
            &mut projector,
            &event(RuntimeEvent::ToolExecutionFailed {
                tool_call_id: ToolCallId::new("call-1"),
                tool_id: ToolId::new("tool-read"),
                error: "denied".to_owned(),
            }),
        );
        assert_eq!(
            projector.observation.activity,
            SubagentActivity::AwaitingActivity
        );
        assert_eq!(projector.observation.counters.tool_executions, 1);
    }

    #[test]
    fn compaction_cycle_projects_and_resets() {
        let mut projector = SubagentObservationProjector::default();
        assert_folded(&mut projector, &event(RuntimeEvent::CompactionStarted));
        assert_eq!(projector.observation.activity, SubagentActivity::Compacting);
        assert_folded(
            &mut projector,
            &event(RuntimeEvent::CompactionFailed {
                error: "no budget".to_owned(),
            }),
        );
        assert_eq!(
            projector.observation.activity,
            SubagentActivity::AwaitingActivity
        );
    }

    #[test]
    fn interactions_project_the_wait_and_settle_to_neutral() {
        let mut projector = SubagentObservationProjector::default();
        let request = crate::runtime::interaction::InteractionRequest {
            id: crate::runtime::identity::InteractionId::new("interaction-1"),
            conversation_id: crate::runtime::identity::ConversationId::new("conv-1"),
            attempt_id: AttemptId::new("attempt-1"),
            turn: 1,
            kind: InteractionKind::Approval {
                call_id: ToolCallId::new("call-1"),
                tool_id: ToolId::new("tool-bash"),
                tool_name: "bash".to_owned(),
                origin: crate::tools::types::ToolOrigin::Builtin,
                mode: crate::tools::types::ToolInvocationMode::Foreground,
                arguments: serde_json::json!({}),
                reason: "policy".to_owned(),
            },
        };
        assert_folded(
            &mut projector,
            &ConversationObservation::InteractionPending {
                request,
                audit: interaction_audit(),
                transcript_cursor: crate::durable::TranscriptCursor::new(1),
            },
        );
        assert_eq!(
            projector.observation.activity,
            SubagentActivity::Waiting {
                on: SubagentWaitReason::Approval {
                    tool_id: ToolId::new("tool-bash"),
                },
            }
        );

        // The settlement returns to neutral even though the wait was the
        // current activity.
        assert_folded(
            &mut projector,
            &ConversationObservation::InteractionSettled {
                interaction_id: crate::runtime::identity::InteractionId::new("interaction-1"),
                outcome: crate::runtime::interaction::InteractionOutcome::Responded {
                    response: crate::runtime::interaction::InteractionResponse::Approval {
                        decision: crate::runtime::interaction::ApprovalDecision::Allow,
                    },
                },
                audit: None,
            },
        );
        assert_eq!(
            projector.observation.activity,
            SubagentActivity::AwaitingActivity
        );
    }

    fn interaction_audit() -> crate::events::types::RuntimeEventEnvelope {
        crate::events::types::RuntimeEventEnvelope {
            schema_version: crate::events::types::EVENT_SCHEMA_VERSION,
            event_id: crate::runtime::identity::EventId::new("event-1"),
            sequence: 1,
            conversation_id: crate::runtime::identity::ConversationId::new("conv-1"),
            attempt_id: Some(AttemptId::new("attempt-1")),
            turn_id: None,
            timestamp: now(),
            event: RuntimeEvent::TurnStarted,
        }
    }

    #[test]
    fn questionnaire_waits_project_without_a_tool_identity() {
        let mut projector = SubagentObservationProjector::default();
        let request = crate::runtime::interaction::InteractionRequest {
            id: crate::runtime::identity::InteractionId::new("interaction-2"),
            conversation_id: crate::runtime::identity::ConversationId::new("conv-1"),
            attempt_id: AttemptId::new("attempt-1"),
            turn: 1,
            kind: InteractionKind::Questionnaire {
                questionnaire: crate::events::interaction::QuestionnaireSpecification {
                    questions: vec![crate::events::interaction::QuestionSpecification {
                        question: "Which?".to_owned(),
                        header: "Pick".to_owned(),
                        options: vec![crate::events::interaction::OptionSpecification {
                            label: "a".to_owned(),
                            description: "first".to_owned(),
                            preview: None,
                        }],
                        multi_select: false,
                    }],
                },
            },
        };
        assert_folded(
            &mut projector,
            &ConversationObservation::InteractionPending {
                request,
                audit: interaction_audit(),
                transcript_cursor: crate::durable::TranscriptCursor::new(1),
            },
        );
        assert_eq!(
            projector.observation.activity,
            SubagentActivity::Waiting {
                on: SubagentWaitReason::Questionnaire,
            }
        );
    }

    #[test]
    fn observations_without_an_activity_signal_are_ignored() {
        let mut projector = SubagentObservationProjector::default();
        assert_ignored(&mut projector, &event(RuntimeEvent::TurnStarted));
        assert_ignored(&mut projector, &ConversationObservation::Shutdown);
    }

    #[test]
    fn settlement_resets_to_neutral_and_keeps_the_counters() {
        let mut projector = SubagentObservationProjector::default();
        assert_folded(
            &mut projector,
            &event(RuntimeEvent::ToolExecutionStarted {
                tool_call_id: ToolCallId::new("call-1"),
                tool_id: ToolId::new("tool-bash"),
            }),
        );
        let counters = projector.observation.counters;
        projector.observation.settle_neutral();
        assert_eq!(
            projector.observation.activity,
            SubagentActivity::AwaitingActivity
        );
        assert_eq!(projector.observation.revision, 2);
        assert_eq!(projector.observation.counters, counters);
        assert_eq!(projector.observation.last_activity_at, Some(now()));
    }

    #[test]
    fn the_projection_round_trips_as_snake_case_json() {
        let mut projector = SubagentObservationProjector::default();
        assert_folded(
            &mut projector,
            &event(RuntimeEvent::ToolExecutionStarted {
                tool_call_id: ToolCallId::new("call-1"),
                tool_id: ToolId::new("tool-bash"),
            }),
        );
        let value = serde_json::to_value(projector.observation()).expect("serialize");
        assert_eq!(value["revision"], 1);
        assert_eq!(value["activity"]["type"], "tool");
        assert_eq!(value["activity"]["tool_call_id"], "call-1");
        assert_eq!(value["counters"]["model_requests"], 0);
        let decoded: SubagentObservation =
            serde_json::from_value(value.clone()).expect("deserialize");
        assert_eq!(&decoded, projector.observation());
        // Unknown fields are rejected outright.
        let mut malformed = value;
        malformed["unexpected"] = serde_json::json!(true);
        assert!(serde_json::from_value::<SubagentObservation>(malformed).is_err());
    }

    #[test]
    fn the_execution_profile_derives_only_the_redacted_model_facts() {
        let frozen = crate::model::frozen::test_frozen_model_spec(
            serde_json::from_value(serde_json::json!("local/model")).expect("model ref"),
        );
        let profile = SubagentExecutionProfile::from_frozen(&frozen);
        assert_eq!(profile.model, "local/model");
        assert_eq!(profile.reasoning_profile, None);
        assert!(!profile.reasoning_enabled);
        let serialized = serde_json::to_string(&profile).expect("serialize");
        assert!(
            !serialized.contains("test-only-secret"),
            "no credential material crosses into the profile: {serialized}"
        );
        assert!(
            !serialized.contains("127.0.0.1"),
            "no endpoint material crosses into the profile: {serialized}"
        );
    }

    fn tool_started(projector: &mut SubagentObservationProjector, call: &str) {
        assert_folded(
            projector,
            &event(RuntimeEvent::ToolExecutionStarted {
                tool_call_id: ToolCallId::new(call),
                tool_id: ToolId::new("tool-bash"),
            }),
        );
    }

    fn live_progress(call: &str, message: &str) -> ConversationObservation {
        ConversationObservation::ToolProgress {
            attempt_id: AttemptId::new("attempt-1"),
            tool_call_id: ToolCallId::new(call),
            tool_id: ToolId::new("tool-bash"),
            progress: ToolProgress {
                message: Some(message.to_owned()),
                ..ToolProgress::default()
            },
        }
    }

    fn tool_completed(call: &str) -> ConversationObservation {
        event(RuntimeEvent::ToolExecutionCompleted {
            tool_call_id: ToolCallId::new(call),
            tool_id: ToolId::new("tool-bash"),
            result: tool_result(),
        })
    }

    fn tool_activity(call: &str, message: Option<&str>) -> SubagentActivity {
        SubagentActivity::Tool {
            tool_call_id: ToolCallId::new(call),
            tool_id: ToolId::new("tool-bash"),
            progress: message.map(|message| ToolProgress {
                message: Some(message.to_owned()),
                ..ToolProgress::default()
            }),
        }
    }

    /// Parallel regression (Issue #178): completing one call of a parallel
    /// group must not reset the projection to neutral while a sibling call
    /// is still objectively executing.
    #[test]
    fn a_parallel_sibling_completion_keeps_the_surviving_call_visible() {
        let mut projector = SubagentObservationProjector::default();
        tool_started(&mut projector, "call-a");
        tool_started(&mut projector, "call-b");
        assert_folded(&mut projector, &live_progress("call-b", "halfway"));

        // call-a settles while call-b still runs: call-b (the call that
        // most recently produced an objective activity fact) stays visible.
        assert_folded(&mut projector, &tool_completed("call-a"));
        assert_eq!(
            projector.observation.activity,
            tool_activity("call-b", Some("halfway")),
            "a surviving parallel call keeps projecting, never neutral"
        );
        assert_eq!(projector.observation.counters.tool_executions, 1);
    }

    /// Parallel regression (Issue #178): when the VISIBLE call settles, the
    /// representative falls back to a deterministic surviving call
    /// (latest-started) instead of going neutral.
    #[test]
    fn a_visible_parallel_completion_falls_back_to_the_survivor() {
        let mut projector = SubagentObservationProjector::default();
        tool_started(&mut projector, "call-a");
        tool_started(&mut projector, "call-b");
        assert_folded(&mut projector, &live_progress("call-b", "halfway"));
        assert_eq!(
            projector.observation.activity,
            tool_activity("call-b", Some("halfway"))
        );

        assert_folded(&mut projector, &tool_completed("call-b"));
        assert_eq!(
            projector.observation.activity,
            tool_activity("call-a", None),
            "the deterministic survivor (latest-started remaining call) becomes visible"
        );
        assert_eq!(projector.observation.counters.tool_executions, 1);
    }

    /// Parallel regression (Issue #178): progress of a call whose sibling
    /// already settled still advances the projection.
    #[test]
    fn progress_of_a_surviving_sibling_remains_observable() {
        let mut projector = SubagentObservationProjector::default();
        tool_started(&mut projector, "call-a");
        tool_started(&mut projector, "call-b");
        assert_folded(&mut projector, &tool_completed("call-a"));

        let revision = projector.observation.revision;
        assert_folded(&mut projector, &live_progress("call-b", "phase 2"));
        assert_eq!(projector.observation.revision, revision + 1);
        assert_eq!(
            projector.observation.activity,
            tool_activity("call-b", Some("phase 2"))
        );
    }

    /// Parallel regression (Issue #178): the projection returns to neutral
    /// only after the FINAL active call settles.
    #[test]
    fn the_projection_goes_neutral_only_after_the_final_parallel_call_settles() {
        let mut projector = SubagentObservationProjector::default();
        tool_started(&mut projector, "call-a");
        tool_started(&mut projector, "call-b");

        assert_folded(&mut projector, &tool_completed("call-a"));
        assert_eq!(
            projector.observation.activity,
            tool_activity("call-b", None),
            "one call is still objectively executing"
        );

        assert_folded(&mut projector, &tool_completed("call-b"));
        assert_eq!(
            projector.observation.activity,
            SubagentActivity::AwaitingActivity,
            "no active call remains"
        );
        assert_eq!(projector.observation.counters.tool_executions, 2);
    }

    /// Parallel regression (Issue #178): two parallel calls count exactly
    /// once each, and a duplicate settlement carries no signal.
    #[test]
    fn parallel_tool_executions_count_exactly_once_each() {
        let mut projector = SubagentObservationProjector::default();
        tool_started(&mut projector, "call-a");
        tool_started(&mut projector, "call-b");
        assert_folded(&mut projector, &tool_completed("call-a"));
        assert_folded(
            &mut projector,
            &event(RuntimeEvent::ToolExecutionFailed {
                tool_call_id: ToolCallId::new("call-b"),
                tool_id: ToolId::new("tool-bash"),
                error: "denied".to_owned(),
            }),
        );
        assert_eq!(projector.observation.counters.tool_executions, 2);

        // A duplicate settlement of call-a is stale: no counter, no
        // revision.
        assert_ignored(&mut projector, &tool_completed("call-a"));
        assert_eq!(projector.observation.counters.tool_executions, 2);
    }

    /// Parallel regression (Issue #178): a late progress report of an
    /// already-settled call can never resurrect it.
    #[test]
    fn stale_progress_never_resurrects_a_completed_call() {
        let mut projector = SubagentObservationProjector::default();
        tool_started(&mut projector, "call-a");
        assert_folded(&mut projector, &tool_completed("call-a"));
        assert_eq!(
            projector.observation.activity,
            SubagentActivity::AwaitingActivity
        );

        assert_ignored(&mut projector, &live_progress("call-a", "late"));
        assert_eq!(
            projector.observation.activity,
            SubagentActivity::AwaitingActivity
        );
    }

    /// Parallel regression (Issue #178): an interaction settlement while a
    /// sibling tool still runs returns to the surviving tool, not to
    /// neutral.
    #[test]
    fn an_interaction_settlement_reprojects_a_still_running_sibling() {
        let mut projector = SubagentObservationProjector::default();
        tool_started(&mut projector, "call-a");
        tool_started(&mut projector, "call-b");
        assert_folded(
            &mut projector,
            &ConversationObservation::InteractionPending {
                request: crate::runtime::interaction::InteractionRequest {
                    id: crate::runtime::identity::InteractionId::new("interaction-1"),
                    conversation_id: crate::runtime::identity::ConversationId::new("conv-1"),
                    attempt_id: AttemptId::new("attempt-1"),
                    turn: 1,
                    kind: InteractionKind::Approval {
                        call_id: ToolCallId::new("call-a"),
                        tool_id: ToolId::new("tool-bash"),
                        tool_name: "bash".to_owned(),
                        origin: crate::tools::types::ToolOrigin::Builtin,
                        mode: crate::tools::types::ToolInvocationMode::Foreground,
                        arguments: serde_json::json!({}),
                        reason: "policy".to_owned(),
                    },
                },
                audit: interaction_audit(),
                transcript_cursor: crate::durable::TranscriptCursor::new(1),
            },
        );
        assert!(matches!(
            projector.observation.activity,
            SubagentActivity::Waiting { .. }
        ));

        assert_folded(
            &mut projector,
            &ConversationObservation::InteractionSettled {
                interaction_id: crate::runtime::identity::InteractionId::new("interaction-1"),
                outcome: crate::runtime::interaction::InteractionOutcome::Responded {
                    response: crate::runtime::interaction::InteractionResponse::Approval {
                        decision: crate::runtime::interaction::ApprovalDecision::Allow,
                    },
                },
                audit: None,
            },
        );
        assert_eq!(
            projector.observation.activity,
            tool_activity("call-b", None),
            "the latest-started still-active call is the representative again"
        );
    }
}
