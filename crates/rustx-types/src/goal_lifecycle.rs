//! Goal durable lifecycle vocabulary: writes, transitions and journal envelopes.

use serde::{Deserialize, Serialize};

use crate::goal::{GoalChange, GoalFact, GoalOrigin, GoalPhase, GoalRef, GoalSnapshot};

/// Default number of driver-admitted autonomous rounds.
pub const DEFAULT_ROUND_BUDGET: u32 = 10;
/// Hard native autonomous-round limit.
pub const MAX_ROUND_BUDGET: u32 = 100;
/// UTF-8 byte bound for user task data.
pub const MAX_OBJECTIVE_BYTES: usize = 8192;

/// Explicit user/control mutations. Model adapters expose only Block/Complete.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
pub enum GoalMutation {
    Pause,
    Resume,
    Block { reason: String },
    Complete,
    Edit { objective: String },
    Budget { rounds: u32 },
}

/// Typed Runtime Client control; an existing-state mutation always names its observation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
pub enum GoalControl {
    Show,
    Create {
        objective: String,
        #[serde(default = "default_control_budget")]
        budget: u32,
    },
    Mutate {
        expected: GoalRef,
        mutation: GoalMutation,
    },
}

fn default_control_budget() -> u32 {
    DEFAULT_ROUND_BUDGET
}

/// Semantic refusal, including the bounded current observation for stale CAS.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoalRejection {
    pub reason: String,
    pub current: Option<GoalSnapshot>,
}

pub type GoalResult = Result<GoalSnapshot, Box<GoalRejection>>;

/// Durable transition inputs; admission is deliberately a separate store seam.
pub enum GoalWrite {
    Create {
        objective: String,
        budget: u32,
        origin: GoalOrigin,
    },
    Mutate {
        expected: GoalRef,
        mutation: GoalMutation,
    },
}

pub fn reject(reason: &str, current: Option<&GoalSnapshot>) -> Box<GoalRejection> {
    Box::new(GoalRejection {
        reason: reason.to_owned(),
        current: current.cloned(),
    })
}

pub fn validate_text(text: &str) -> bool {
    !text.trim().is_empty() && text.len() <= MAX_OBJECTIVE_BYTES
}

/// Pure state machine, called within the durable write transaction.
pub fn transition(current: Option<&GoalSnapshot>, write: GoalWrite) -> GoalResult {
    match write {
        GoalWrite::Create {
            objective,
            budget,
            origin,
        } => {
            if current
                .as_ref()
                .is_some_and(|g| g.phase != GoalPhase::Complete)
            {
                return Err(reject("An unfinished Goal already exists", current));
            }
            if !validate_text(&objective) || !(1..=MAX_ROUND_BUDGET).contains(&budget) {
                return Err(reject(
                    "Objective or autonomous-round budget is out of bounds",
                    current,
                ));
            }
            let revision = current
                .as_ref()
                .map_or(Some(1), |g| g.reference.revision.checked_add(1))
                .ok_or_else(|| reject("Goal revision exhausted", current))?;
            Ok(GoalSnapshot {
                reference: GoalRef {
                    id: format!("goal-{revision}"),
                    revision,
                },
                objective,
                phase: GoalPhase::Active,
                blocked_reason: None,
                autonomous_round_budget: budget,
                autonomous_rounds_consumed: 0,
                origin,
                last_round_message_id: None,
            })
        }
        GoalWrite::Mutate { expected, mutation } => {
            let Some(mut goal) = current.cloned() else {
                return Err(reject("No current Goal", current));
            };
            if goal.reference != expected {
                return Err(reject(
                    "Stale GoalRef; observe current state before trying again",
                    current,
                ));
            }
            if goal.phase == GoalPhase::Complete {
                return Err(reject("Complete is terminal", current));
            }
            match mutation {
                GoalMutation::Pause if goal.phase == GoalPhase::Active => {
                    goal.phase = GoalPhase::Paused;
                }
                // Resume is a real state transition, never a re-arm:
                // `Active -> Active` no longer exists, so a resume against an
                // already-Active Goal is rejected rather than silently
                // advancing the revision.
                GoalMutation::Resume
                    if matches!(goal.phase, GoalPhase::Paused | GoalPhase::Blocked) =>
                {
                    goal.phase = GoalPhase::Active;
                    goal.blocked_reason = None;
                }
                GoalMutation::Block { reason }
                    if goal.phase == GoalPhase::Active && validate_text(&reason) =>
                {
                    goal.phase = GoalPhase::Blocked;
                    goal.blocked_reason = Some(reason);
                }
                GoalMutation::Complete if goal.phase == GoalPhase::Active => {
                    goal.phase = GoalPhase::Complete;
                }
                GoalMutation::Edit { objective } if validate_text(&objective) => {
                    goal.objective = objective;
                }
                GoalMutation::Budget { rounds }
                    if (1..=MAX_ROUND_BUDGET).contains(&rounds)
                        && rounds >= goal.autonomous_rounds_consumed =>
                {
                    goal.autonomous_round_budget = rounds;
                }
                _ => return Err(reject("Invalid Goal transition or value", current)),
            }
            goal.reference.revision = goal
                .reference
                .revision
                .checked_add(1)
                .ok_or_else(|| reject("Goal revision exhausted", current))?;
            Ok(goal)
        }
    }
}

impl GoalWrite {
    pub fn change(&self) -> GoalChange {
        match self {
            Self::Create { .. } => GoalChange::Create,
            Self::Mutate { mutation, .. } => match mutation {
                GoalMutation::Pause => GoalChange::Pause,
                GoalMutation::Resume => GoalChange::Resume,
                GoalMutation::Block { .. } => GoalChange::Block,
                GoalMutation::Complete => GoalChange::Complete,
                GoalMutation::Edit { .. } => GoalChange::Edit,
                GoalMutation::Budget { .. } => GoalChange::Budget,
            },
        }
    }
}

pub fn journal_envelope(
    conversation_id: &rustx_identity::ConversationId,
    fact: GoalFact,
) -> crate::events::types::RuntimeEventEnvelope {
    crate::events::types::RuntimeEventEnvelope {
        tool_occurrence: None,
        observation: None,
        schema_version: crate::events::types::EVENT_SCHEMA_VERSION,
        event_id: rustx_identity::EventId::new(""),
        sequence: 0,
        conversation_id: conversation_id.clone(),
        attempt_id: None,
        turn_id: None,
        timestamp: chrono::Utc::now(),
        event: crate::events::types::RuntimeEvent::Goal { fact },
    }
}

/// activation member, so `Active + inert` is unrepresentable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct GoalView {
    pub current: Option<GoalSnapshot>,
}
