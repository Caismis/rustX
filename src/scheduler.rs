//! Durable temporal eligibility. Execution belongs to the ordinary target owner.
use crate::runtime::identity::{ConversationId, MessageId};
use crate::runtime::inbound::InboundSequence;
use chrono::{DateTime, Datelike, Duration, Utc};
use serde::{Deserialize, Serialize};
mod calendar;
pub(crate) mod control;

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
/// Exactly one admitted native target, never a generic callback.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ScheduleTarget {
    ConversationInbound {
        text: String,
    },
    Workflow {
        #[schemars(with = "serde_json::Value")]
        pinned: Box<crate::runtime::workflow::scheduled::PinnedScheduledWorkflow>,
        arguments: serde_json::Value,
    },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ScheduleTargetHeader {
    ConversationInbound,
    Workflow {
        workflow_id: crate::runtime::workflow::WorkflowId,
    },
}
impl ScheduleTarget {
    #[must_use]
    pub fn header(&self) -> ScheduleTargetHeader {
        match self {
            Self::ConversationInbound { .. } => ScheduleTargetHeader::ConversationInbound,
            Self::Workflow { pinned, .. } => ScheduleTargetHeader::Workflow {
                workflow_id: pinned.definition.workflow_id.clone(),
            },
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ScheduledTargetReceipt {
    ConversationInbound {
        message_id: MessageId,
        sequence: InboundSequence,
    },
    Workflow {
        start_id: crate::runtime::workflow::scheduled::lifecycle::ScheduledWorkflowStartId,
        run_id: crate::runtime::workflow::WorkflowRunId,
    },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScheduleTriggerReceipt {
    pub trigger_id: ScheduleTriggerId,
    pub accepted_at: DateTime<Utc>,
    pub target: ScheduledTargetReceipt,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScheduleSnapshot {
    pub id: ScheduleId,
    pub binding: ScheduleBinding,
    pub revision: u64,
    pub title: String,
    pub target: ScheduleTarget,
    pub spec: ScheduleSpec,
    pub state: ScheduleState,
    pub next_due_utc: Option<DateTime<Utc>>,
    pub last_trigger: Option<ScheduleTriggerReceipt>,
    pub completed_at: Option<DateTime<Utc>>,
    pub completion_reason: Option<ScheduleCompletionReason>,
    pub last_failure: Option<String>,
}

/// Bounded product list projection. Reminder text is read through exact Get.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScheduleHeader {
    pub id: ScheduleId,
    pub binding: ScheduleBinding,
    pub revision: u64,
    pub title: String,
    pub target: ScheduleTargetHeader,
    pub spec: ScheduleSpec,
    pub state: ScheduleState,
    pub next_due_utc: Option<DateTime<Utc>>,
    pub last_trigger: Option<ScheduleTriggerReceipt>,
    pub completed_at: Option<DateTime<Utc>>,
    pub completion_reason: Option<ScheduleCompletionReason>,
    pub last_failure: Option<String>,
}

impl From<ScheduleSnapshot> for ScheduleHeader {
    fn from(snapshot: ScheduleSnapshot) -> Self {
        let ScheduleSnapshot {
            id,
            binding,
            revision,
            title,
            target,
            spec,
            state,
            next_due_utc,
            last_trigger,
            completed_at,
            completion_reason,
            last_failure,
        } = snapshot;
        Self {
            id,
            binding,
            revision,
            title,
            target: target.header(),
            spec,
            state,
            next_due_utc,
            last_trigger,
            completed_at,
            completion_reason,
            last_failure,
        }
    }
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
    CreateWorkflow {
        title: String,
        workflow_id: crate::runtime::workflow::WorkflowId,
        arguments: serde_json::Value,
        timing: ScheduleTiming,
    },
    UpdateWorkflow {
        id: ScheduleId,
        expected_revision: u64,
        title: String,
        workflow_id: crate::runtime::workflow::WorkflowId,
        arguments: serde_json::Value,
        timing: Option<ScheduleTiming>,
    },

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
    WorkflowAdmitted {
        snapshot: ScheduleSnapshot,
        start: Box<crate::runtime::workflow::scheduled::lifecycle::ScheduledWorkflowHead>,
    },

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
    /// Native pending Messages and unclaimed Workflows; never replay Claimed work.
    pub ready: Vec<ScheduleTriggerId>,
}

/// Closed native storage operations. These never lend a store or runtime.
#[derive(Debug, Clone)]
pub(crate) enum ScheduleOperation {
    WorkflowRecovery {
        id: crate::runtime::workflow::scheduled::lifecycle::ScheduledWorkflowStartId,
        after: u64,
        limit: u32,
    },
    WorkflowCancel {
        id: crate::runtime::workflow::scheduled::lifecycle::ScheduledWorkflowStartId,
    },
    WorkflowReconcile {
        id: crate::runtime::workflow::scheduled::lifecycle::ScheduledWorkflowStartId,
        after: u64,
        limit: u32,
    },
    WorkflowReconcileOwned {
        id: crate::runtime::workflow::scheduled::lifecycle::ScheduledWorkflowStartId,
        after: u64,
        limit: u32,
        retirement:
            crate::runtime::workflow::scheduled::lifecycle::ScheduledWorkflowOwnerRetirement,
    },

    WorkflowCommand {
        command: ScheduleCommand,
        contract: Box<crate::runtime::workflow::scheduled::PinnedScheduledWorkflow>,
    },

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
    WorkflowRecovery(
        Option<crate::runtime::workflow::scheduled::lifecycle::ScheduledWorkflowRecovery>,
    ),

    Command(ScheduleCommandResult),
    Get(Option<ScheduleSnapshot>),
    List(Vec<ScheduleSnapshot>),
    Summary(ScheduleSummary),
    DuePage(ScheduleDuePage),
    Pruned {
        removed: usize,
        generation: u64,
    },
    Admission(ScheduleAdmission),
}
impl ScheduleOperation {
    #[allow(clippy::too_many_lines)] // Closed native store operation dispatch.
    pub(crate) fn execute(
        &self,
        store: &dyn crate::durable::ConversationStore,
        binding: &ScheduleBinding,
        clock: &dyn crate::runtime::types::RuntimeClock,
    ) -> Result<ScheduleOperationResult, crate::durable::ConversationStoreError> {
        Ok(match self {
            Self::WorkflowRecovery { id, after, limit } => {
                let view = store
                    .scheduled_workflow_recovery(id, *after, *limit)?
                    .filter(|view| view.start.binding == *binding);
                ScheduleOperationResult::WorkflowRecovery(view)
            }
            Self::WorkflowCancel { id } => {
                if store
                    .scheduled_workflow_get(id)?
                    .is_some_and(|start| start.head.binding == *binding)
                {
                    store.scheduled_workflow_cancel(id, clock.now())?;
                }
                ScheduleOperationResult::WorkflowRecovery(
                    store
                        .scheduled_workflow_recovery(id, 0, 100)?
                        .filter(|view| view.start.binding == *binding),
                )
            }
            Self::WorkflowReconcile { id, after, limit } => {
                // This entrance is used only by manager-owned StorageOnly.
                // A live runtime substitutes its exact native owner proof.
                let retirement=crate::runtime::workflow::scheduled::lifecycle::ScheduledWorkflowOwnerRetirement{start_id:id.clone(),recovered_writer:true,agent_domain_quiescent:false,interactions_closed:true};
                return Self::WorkflowReconcileOwned {
                    id: id.clone(),
                    after: *after,
                    limit: *limit,
                    retirement,
                }
                .execute(store, binding, clock);
            }
            Self::WorkflowReconcileOwned {
                id,
                after,
                limit,
                retirement,
            } => {
                use crate::runtime::workflow::scheduled::lifecycle::{
                    QuiescenceSource, ScheduledWorkflowOutcome, ScheduledWorkflowQuiescence,
                    ScheduledWorkflowSettlement, ScheduledWorkflowState,
                };
                if let Some(start) = store
                    .scheduled_workflow_get(id)?
                    .filter(|start| start.head.binding == *binding)
                    && matches!(start.head.state, ScheduledWorkflowState::Claimed { .. })
                {
                    store.scheduled_workflow_recover_effects(id, *after, *limit, retirement)?;
                    let source = if matches!(
                        start.head.state,
                        ScheduledWorkflowState::Claimed {
                            effects_started: false,
                            ..
                        }
                    ) {
                        QuiescenceSource::RecoveredNoEffects
                    } else {
                        QuiescenceSource::RecoveredOwners
                    };
                    if store.scheduled_workflow_settle(id,ScheduledWorkflowOutcome::OutcomeUnknown{reason:"native ownership retired after interruption; graph was not replayed".into()},ScheduledWorkflowQuiescence{source},None,clock.now())?==ScheduledWorkflowSettlement::AwaitingNativeProof {
                        store.scheduled_workflow_blocked(id,"recovery_blocked: exact native owner proof is missing; inspect effect owners and use only their supported cancellation/recovery controls")?;
                    }
                }
                ScheduleOperationResult::WorkflowRecovery(
                    store
                        .scheduled_workflow_recovery(id, *after, *limit)?
                        .filter(|view| view.start.binding == *binding),
                )
            }

            Self::WorkflowCommand { command, contract } => {
                ScheduleOperationResult::Command(store.schedule_workflow_command(
                    binding,
                    command,
                    clock,
                    &ScheduleCommandAuthority::RuntimeControl,
                    contract,
                )?)
            }

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
            } => ScheduleOperationResult::Admission(store.admit_scheduled_target(
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
