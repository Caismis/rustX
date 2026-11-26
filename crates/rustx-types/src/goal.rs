//! Durable Goal lifecycle types.
use crate::runtime::identity::{AttemptId, MessageId};
use serde::{Deserialize, Serialize};

/// Exact observation required by every mutation of an existing Goal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GoalRef {
    pub id: String,
    pub revision: u64,
}

/// The single durable Goal lifecycle authority.
///
/// - `Active`: continuation is authorized when runtime admission is eligible.
/// - `Paused` / `Blocked`: continuation is not authorized.
/// - `Complete`: terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(schemars::JsonSchema)]
pub enum GoalPhase {
    Active,
    Paused,
    Blocked,
    Complete,
}

/// Trusted origin supplied by runtime, never by model arguments.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
pub enum GoalOrigin {
    HumanAttempt {
        message_id: MessageId,
        attempt_id: AttemptId,
    },
    RuntimeControl,
}

/// Authoritative bounded durable record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
pub struct GoalSnapshot {
    pub reference: GoalRef,
    pub objective: String,
    pub phase: GoalPhase,
    pub blocked_reason: Option<String>,
    pub autonomous_round_budget: u32,
    pub autonomous_rounds_consumed: u32,
    pub origin: GoalOrigin,
    pub last_round_message_id: Option<MessageId>,
}

/// Bounded audit vocabulary; no objective/history and no recovery authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum GoalFact {
    Written {
        previous: Option<GoalRef>,
        current: GoalRef,
        phase: GoalPhase,
        change: GoalChange,
    },
    RoundAdmitted {
        previous: GoalRef,
        current: GoalRef,
        round: u32,
        message_id: MessageId,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GoalChange {
    Create,
    Pause,
    Resume,
    Block,
    Complete,
    Edit,
    Budget,
}
