//! Durable temporal eligibility. Execution belongs to the ordinary target owner.
use crate::runtime::identity::{ConversationId, MessageId};
use crate::runtime::inbound::InboundSequence;
use chrono::{DateTime, Datelike, Duration, Utc};
use serde::{Deserialize, Serialize};
mod calendar;

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
    Cron {
        expression: String,
        time_zone: String,
    },
}

/// Authored time, before the first successful command resolves relative input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ScheduleTiming {
    At {
        at: DateTime<chrono::FixedOffset>,
    },
    After {
        seconds: u32,
    },
    Interval {
        every_seconds: u32,
    },
    Cron {
        expression: String,
        time_zone: String,
    },
}
impl ScheduleTiming {
    pub(crate) fn input_bounded(&self) -> bool {
        match self {
            Self::Cron {
                expression,
                time_zone,
            } => expression.len() <= 256 && time_zone.len() <= 128,
            _ => true,
        }
    }
    pub(crate) fn resolve(
        &self,
        now: DateTime<Utc>,
    ) -> Result<(ScheduleSpec, DateTime<Utc>), String> {
        match self {
            Self::At { at }
                if at.with_timezone(&Utc) > now && at.with_timezone(&Utc).year() <= 9999 =>
            {
                let at_utc = at.with_timezone(&Utc);
                Ok((ScheduleSpec::At { at_utc }, at_utc))
            }
            Self::After { seconds } if *seconds > 0 => {
                let at_utc = add_seconds(now, i64::from(*seconds))?;
                Ok((ScheduleSpec::At { at_utc }, at_utc))
            }
            Self::Interval { every_seconds } if *every_seconds >= 60 => {
                let anchor_utc = add_seconds(now, i64::from(*every_seconds))?;
                Ok((
                    ScheduleSpec::Interval {
                        anchor_utc,
                        every_seconds: *every_seconds,
                    },
                    anchor_utc,
                ))
            }
            Self::Cron {
                expression,
                time_zone,
            } => {
                let (expression, time_zone, first) = calendar::prepare(expression, time_zone, now)?;
                Ok((
                    ScheduleSpec::Cron {
                        expression,
                        time_zone,
                    },
                    first,
                ))
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
    pub(crate) fn completion_reason(&self) -> ScheduleCompletionReason {
        match self {
            Self::At { .. } => ScheduleCompletionReason::OneShotAdmitted,
            Self::Cron { .. } => ScheduleCompletionReason::CalendarExhausted,
            Self::Interval { .. } => ScheduleCompletionReason::TimeRangeExhausted,
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
            Self::Cron {
                expression,
                time_zone,
            } => calendar::occurrence(expression, time_zone, due, now),
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ScheduleCompletionReason {
    OneShotAdmitted,
    CalendarExhausted,
    TimeRangeExhausted,
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
    pub completion_reason: Option<ScheduleCompletionReason>,
    pub last_failure: Option<String>,
}

/// Trusted native authorization input, never decoded from model/transport fields.
/// The runtime must verify live root scope and fresh Human provenance before
/// supplying `HumanRequest`. This backend value does not establish that provenance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScheduleCommandAuthority {
    RuntimeControl,
    HumanRequest {
        message_id: MessageId,
        original_attempt_id: crate::runtime::identity::AttemptId,
    },
}
pub const SCHEDULE_REQUEST_MUTATION_LIMIT: u8 = 8;
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScheduleRequestBudget {
    pub human_message_id: MessageId,
    pub original_attempt_id: crate::runtime::identity::AttemptId,
    pub mutations_used: u8,
    pub mutation_limit: u8,
}

/// Compact committed command result, retained independently of display history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScheduleCommandReceipt {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_budget: Option<ScheduleRequestBudget>,
    pub id: ScheduleId,
    pub revision: u64,
    pub spec: ScheduleSpec,
    pub state: ScheduleState,
    pub next_due_utc: Option<DateTime<Utc>>,
}
impl From<&ScheduleSnapshot> for ScheduleCommandReceipt {
    fn from(snapshot: &ScheduleSnapshot) -> Self {
        Self {
            request_budget: None,
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
    Applied {
        receipt: ScheduleCommandReceipt,
    },
    Deleted {
        id: ScheduleId,
        revision: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        request_budget: Option<ScheduleRequestBudget>,
    },
    Conflict {
        current: Box<ScheduleSnapshot>,
    },
    NotFound,
    Invalid {
        reason: String,
    },
    Capacity,
    MutationBudgetExhausted,
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
    Disabled {
        current: ScheduleSnapshot,
    },
    NotDue,
    WaitingForTarget,
    SchedulerCapacity,
    NotFound,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduleDueCursor {
    pub due_utc: DateTime<Utc>,
    pub ordinal: u64,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduleCandidate {
    pub id: ScheduleId,
    pub revision: u64,
    pub due_utc: DateTime<Utc>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduleDuePage {
    pub generation: u64,
    pub candidates: Vec<ScheduleCandidate>,
    pub next: Option<ScheduleDueCursor>,
}

/// Header probe from one consistent read transaction. No transcript is loaded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduleSummary {
    pub generation: u64,
    pub next_due_utc: Option<DateTime<Utc>>,
    pub enabled: u64,
    pub oldest_completed_utc: Option<DateTime<Utc>>,
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
    DuePage {
        now: DateTime<Utc>,
        after: Option<ScheduleDueCursor>,
    },
    Prune,
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
    DuePage(ScheduleDuePage),
    Pruned { removed: usize, generation: u64 },
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
            Self::Command(command) => ScheduleOperationResult::Command(store.schedule_command(
                binding,
                command,
                clock,
                &ScheduleCommandAuthority::RuntimeControl,
            )?),
            Self::Get(id) => ScheduleOperationResult::Get(store.schedule_get(id)?),
            Self::List {
                after_ordinal,
                limit,
            } => ScheduleOperationResult::List(store.schedule_list(*after_ordinal, *limit)?),
            Self::DuePage { now, after } => {
                ScheduleOperationResult::DuePage(store.schedule_due_page(*now, after.as_ref())?)
            }
            Self::Prune => {
                let (removed, generation) = store.prune_schedules(clock)?;
                ScheduleOperationResult::Pruned {
                    removed,
                    generation,
                }
            }
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

/// Last native observation and bounded diagnostic for one exact destination.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduleSourceHealth {
    pub binding: ScheduleBinding,
    pub available: bool,
    pub last_success_utc: Option<DateTime<Utc>>,
    pub last_generation: Option<u64>,
    pub next_due_utc: Option<DateTime<Utc>>,
    pub pending_targets: usize,
    pub enabled: u64,
    pub error: Option<String>,
    pub execution_wait: Option<String>,
}
/// Process-local scan health. Durable records remain scheduling authority.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScheduleDriverDiagnostics {
    pub passes_completed: u64,
    pub stores_visited: u64,
    pub inventory_size: usize,
    pub scanned_in_pass: usize,
    pub indexed_conversations: usize,
    pub last_pass_ms: u64,
    pub pass_bound_ms: u64,
    pub discovery_bound_ms: u64,
    pub read_timeouts: u64,
    pub dispatch_failures: u64,
    pub dropped_hints: u64,
    pub inventory_error: Option<String>,
    pub pass_overruns: u64,
    pub sources: std::collections::BTreeMap<ConversationId, ScheduleSourceHealth>,
}
