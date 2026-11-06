//! Workflow-owned durable start and physical-settlement contracts.
use crate::runtime::identity::{AuxiliaryCallId, SubagentId};
use crate::runtime::workflow::{WorkflowNodeInstance, WorkflowRunId};
use crate::scheduler::{ScheduleBinding, ScheduleTriggerId};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(try_from = "String", into = "String")]
pub struct ScheduledWorkflowStartId(String);
impl ScheduledWorkflowStartId {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
    pub(crate) fn for_trigger(trigger: &ScheduleTriggerId) -> Self {
        use sha2::{Digest, Sha256};
        Self(format!(
            "swf_{:x}",
            Sha256::digest(serde_json::to_vec(trigger).expect("native TriggerId"))
        ))
    }
}
impl std::fmt::Display for ScheduledWorkflowStartId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl TryFrom<String> for ScheduledWorkflowStartId {
    type Error = String;
    fn try_from(value: String) -> Result<Self, String> {
        if value.len() == 68
            && value.starts_with("swf_")
            && value[4..]
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        {
            Ok(Self(value))
        } else {
            Err("invalid native scheduled start identity".into())
        }
    }
}
impl From<ScheduledWorkflowStartId> for String {
    fn from(value: ScheduledWorkflowStartId) -> Self {
        value.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ScheduledWorkflowOutcome {
    Completed,
    Failed { reason: String },
    Cancelled,
    TimedOut,
    RejectedBeforeStart { reason: String },
    Interrupted { reason: String },
    OutcomeUnknown { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ScheduledWorkflowState {
    Accepted {
        waiting: Option<String>,
        retry_after: Option<DateTime<Utc>>,
    },
    Claimed {
        owner: String,
        claimed_at: DateTime<Utc>,
        effects_started: bool,
    },
    Settled {
        outcome: ScheduledWorkflowOutcome,
        settled_at: DateTime<Utc>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScheduledWorkflowHead {
    pub id: ScheduledWorkflowStartId,
    pub run_id: WorkflowRunId,
    pub trigger_id: ScheduleTriggerId,
    pub binding: ScheduleBinding,
    pub accepted_at: DateTime<Utc>,
    pub state: ScheduledWorkflowState,
    pub cancellation_requested_at: Option<DateTime<Utc>>,
    pub recovery_blocked: Option<String>,
    pub native_quiescence_proven: bool,
    pub result_digest: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScheduledWorkflowStart {
    pub head: ScheduledWorkflowHead,
    pub contract: Option<super::PinnedScheduledWorkflow>,
    pub arguments: Option<serde_json::Value>,
    pub result: Option<serde_json::Value>,
}

/// Issued once by Accepted->Claimed. Deserialization cannot manufacture it.
#[derive(Debug)]
pub struct ClaimedScheduledWorkflow {
    pub(crate) start: ScheduledWorkflowStart,
}
impl ClaimedScheduledWorkflow {
    #[must_use]
    pub fn start(&self) -> &ScheduledWorkflowStart {
        &self.start
    }
    pub(crate) fn owner(&self) -> &str {
        match &self.start.head.state {
            ScheduledWorkflowState::Claimed { owner, .. } => owner,
            _ => unreachable!("native claim capability"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ScheduledEffectKind {
    Tool,
    Agent,
    Model,
    Review,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ScheduledEffectOwner {
    Preparing,
    Tool {
        invocation: crate::tools::types::ToolInvocationId,
    },
    Agent {
        activation: SubagentId,
    },
    Model {
        call_id: AuxiliaryCallId,
    },
    Review,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScheduledWorkflowEffect {
    pub ordinal: u64,
    pub node: WorkflowNodeInstance,
    pub kind: ScheduledEffectKind,
    pub owner: ScheduledEffectOwner,
    pub physical_settlement_proven: bool,
    pub diagnostic: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScheduledWorkflowRecovery {
    pub start: ScheduledWorkflowHead,
    pub effects: Vec<ScheduledWorkflowEffect>,
    pub result: Option<serde_json::Value>,
    pub next_effect_ordinal: Option<u64>,
    pub capacity_blocked: bool,
    pub deletion_blocked: bool,
}

/// Native-only proof, produced after the ordinary physical owners settle.
/// Operator input has no corresponding constructor or decoding surface.
pub struct ScheduledWorkflowQuiescence {
    pub(crate) source: QuiescenceSource,
}
pub(crate) enum QuiescenceSource {
    NativeExecutor,
    RecoveredOwners,
    RecoveredNoEffects,
}

pub(crate) fn bounded_reason(reason: impl AsRef<str>) -> String {
    reason.as_ref().chars().take(256).collect()
}

/// Captured by native runtime/storage ownership, never accepted from clients.
#[derive(Debug, Clone)]
pub struct ScheduledWorkflowOwnerRetirement {
    pub(crate) start_id: ScheduledWorkflowStartId,
    pub(crate) recovered_writer: bool,
    pub(crate) agent_domain_quiescent: bool,
    pub(crate) interactions_closed: bool,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScheduledWorkflowSettlement {
    Settled,
    AlreadySettled,
    AwaitingNativeProof,
}
