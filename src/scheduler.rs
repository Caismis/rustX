//! Durable temporal eligibility. Execution belongs to the ordinary target owner.
use crate::runtime::identity::{ConversationId, MessageId};
use crate::runtime::inbound::InboundSequence;
use chrono::{DateTime, Datelike, Duration, Utc};
use serde::{Deserialize, Serialize};

/// A conversation-scoped identity whose ordinal is never reused.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScheduleId {
    pub conversation_id: ConversationId,
    pub ordinal: u64,
}

/// Native destination; changing it requires a new schedule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScheduleBinding {
    pub session_id: crate::local_runtime::session::SessionId,
    pub node_id: crate::local_runtime::session::SessionNodeId,
    pub conversation_id: ConversationId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ScheduleSpec {
    At {
        at_utc: DateTime<Utc>,
    },
    Interval {
        anchor_utc: DateTime<Utc>,
        every_seconds: u32,
    },
}

/// Authored time, before the first successful command resolves relative input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ScheduleTiming {
    At { at: DateTime<chrono::FixedOffset> },
    After { seconds: u32 },
    Interval { every_seconds: u32 },
}
impl ScheduleTiming {
    pub(crate) fn resolve(&self, now: DateTime<Utc>) -> Result<ScheduleSpec, String> {
        match self {
            Self::At { at }
                if at.with_timezone(&Utc) > now && at.with_timezone(&Utc).year() <= 9999 =>
            {
                Ok(ScheduleSpec::At {
                    at_utc: at.with_timezone(&Utc),
                })
            }
            Self::After { seconds } if *seconds > 0 => Ok(ScheduleSpec::At {
                at_utc: add_seconds(now, i64::from(*seconds))?,
            }),
            Self::Interval { every_seconds } if *every_seconds >= 60 => {
                Ok(ScheduleSpec::Interval {
                    anchor_utc: add_seconds(now, i64::from(*every_seconds))?,
                    every_seconds: *every_seconds,
                })
            }
            _ => Err(
                "At must be future; after must be positive; interval must be at least 60 seconds"
                    .into(),
            ),
        }
    }
}
fn add_seconds(at: DateTime<Utc>, seconds: i64) -> Result<DateTime<Utc>, String> {
    at.checked_add_signed(Duration::seconds(seconds))
        .filter(|instant| instant.year() <= 9999)
        .ok_or_else(|| "schedule time exhausted".into())
}
impl ScheduleSpec {
    pub(crate) fn first(&self) -> DateTime<Utc> {
        match self {
            Self::At { at_utc } => *at_utc,
            Self::Interval { anchor_utc, .. } => *anchor_utc,
        }
    }
    /// Constant-time latest-only catch-up, aligned to the committed anchor.
    pub(crate) fn occurrence(
        &self,
        due: DateTime<Utc>,
        now: DateTime<Utc>,
    ) -> Result<(DateTime<Utc>, Option<DateTime<Utc>>), String> {
        if now < due {
            return Err("occurrence is not due".into());
        }
        match self {
            Self::At { .. } => Ok((due, None)),
            Self::Interval { every_seconds, .. } => {
                let period = i64::from(*every_seconds);
                if period < 60 {
                    return Err("invalid stored interval".into());
                }
                let elapsed = now.signed_duration_since(due).num_seconds();
                let selected = add_seconds(
                    due,
                    (elapsed / period)
                        .checked_mul(period)
                        .ok_or("schedule time exhausted")?,
                )?;
                Ok((
                    selected,
                    selected
                        .checked_add_signed(Duration::seconds(period))
                        .filter(|instant| instant.year() <= 9999),
                ))
            }
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ScheduleState {
    Enabled,
    Disabled,
    Completed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScheduleTriggerId {
    pub schedule_id: ScheduleId,
    pub admission_revision: u64,
    pub occurrence_utc: DateTime<Utc>,
}
impl ScheduleTriggerId {
    pub(crate) fn correlation(&self) -> String {
        format!(
            "scheduler:{}:{}:{}",
            self.schedule_id.ordinal,
            self.admission_revision,
            self.occurrence_utc.to_rfc3339()
        )
    }
    pub(crate) fn message_id(&self) -> MessageId {
        MessageId::new(format!(
            "scheduler:{}:{}",
            self.schedule_id.conversation_id.as_str(),
            self.correlation()
        ))
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScheduleTriggerReceipt {
    pub trigger_id: ScheduleTriggerId,
    pub accepted_at: DateTime<Utc>,
    pub message_id: MessageId,
    pub sequence: InboundSequence,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScheduleSnapshot {
    pub id: ScheduleId,
    pub binding: ScheduleBinding,
    pub revision: u64,
    pub title: String,
    pub text: String,
    pub spec: ScheduleSpec,
    pub state: ScheduleState,
    pub next_due_utc: Option<DateTime<Utc>>,
    pub last_trigger: Option<ScheduleTriggerReceipt>,
    pub completed_at: Option<DateTime<Utc>>,
}

/// Compact committed command result, retained independently of display history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScheduleCommandReceipt {
    pub id: ScheduleId,
    pub revision: u64,
    pub spec: ScheduleSpec,
    pub state: ScheduleState,
    pub next_due_utc: Option<DateTime<Utc>>,
}
impl From<&ScheduleSnapshot> for ScheduleCommandReceipt {
    fn from(snapshot: &ScheduleSnapshot) -> Self {
        Self {
            id: snapshot.id.clone(),
            revision: snapshot.revision,
            spec: snapshot.spec.clone(),
            state: snapshot.state,
            next_due_utc: snapshot.next_due_utc,
        }
    }
}

/// Trusted product-control commands. Model request grants are a separate seam.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ScheduleMutation {
    Create {
        title: String,
        text: String,
        timing: ScheduleTiming,
    },
    Update {
        id: ScheduleId,
        expected_revision: u64,
        title: String,
        text: String,
        timing: Option<ScheduleTiming>,
    },
    SetEnabled {
        id: ScheduleId,
        expected_revision: u64,
        enabled: bool,
    },
    Delete {
        id: ScheduleId,
        expected_revision: u64,
    },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScheduleCommand {
    pub command_id: String,
    pub mutation: ScheduleMutation,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum ScheduleCommandResult {
    Applied { receipt: ScheduleCommandReceipt },
    Deleted { id: ScheduleId, revision: u64 },
    Conflict { current: Box<ScheduleSnapshot> },
    NotFound,
    Invalid { reason: String },
    Capacity,
}
#[derive(Debug, Clone)]
pub enum ScheduleAdmission {
    Admitted {
        snapshot: ScheduleSnapshot,
        inbound: Box<crate::durable::AcceptedInbound>,
    },
    AlreadyAdvanced {
        current: ScheduleSnapshot,
    },
    NotDue,
    WaitingForTarget,
    SchedulerCapacity,
    NotFound,
}
/// Header probe from one consistent read transaction. No transcript is loaded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduleSummary {
    pub generation: u64,
    pub next_due_utc: Option<DateTime<Utc>>,
    pub enabled: u64,
    pub pending: Vec<ScheduleTriggerId>,
}

/// Closed native storage operations. These never lend a store or runtime.
#[derive(Debug, Clone)]
pub(crate) enum ScheduleOperation {
    Command(ScheduleCommand),
    Get(ScheduleId),
    List {
        after_ordinal: u64,
        limit: u32,
    },
    Summary,
    Recover,
    Admit {
        id: ScheduleId,
        expected_revision: u64,
    },
}
#[derive(Debug, Clone)]
pub(crate) enum ScheduleOperationResult {
    Command(ScheduleCommandResult),
    Get(Option<ScheduleSnapshot>),
    List(Vec<ScheduleSnapshot>),
    Summary(ScheduleSummary),
    Admission(ScheduleAdmission),
}
impl ScheduleOperation {
    pub(crate) fn execute(
        &self,
        store: &dyn crate::durable::ScheduleStore,
        binding: &ScheduleBinding,
        clock: &dyn crate::runtime::types::RuntimeClock,
    ) -> Result<ScheduleOperationResult, crate::durable::ConversationStoreError> {
        Ok(match self {
            Self::Command(command) => {
                ScheduleOperationResult::Command(store.schedule_command(binding, command, clock)?)
            }
            Self::Get(id) => ScheduleOperationResult::Get(store.schedule_get(id)?),
            Self::List {
                after_ordinal,
                limit,
            } => ScheduleOperationResult::List(store.schedule_list(*after_ordinal, *limit)?),
            Self::Summary | Self::Recover => {
                ScheduleOperationResult::Summary(store.schedule_summary()?)
            }
            Self::Admit {
                id,
                expected_revision,
            } => ScheduleOperationResult::Admission(store.admit_scheduled_inbound(
                id,
                *expected_revision,
                clock,
            )?),
        })
    }
}

/// Process-local scan health. Durable records remain the scheduling authority.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScheduleDriverDiagnostics {
    pub passes_completed: u64,
    pub stores_visited: u64,
    pub inventory_size: usize,
    pub indexed_conversations: usize,
    pub last_pass_ms: u64,
    pub unavailable: std::collections::BTreeMap<ConversationId, String>,
}
