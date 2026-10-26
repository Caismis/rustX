//! Schedule CAS and native inbound acceptance share one immediate transaction.
use super::{SqliteConversationStore, decode, encode, storage};
use crate::durable::{ConversationStoreError, InboundDraft, ScheduleStore};
use crate::message::{
    content::TextBlock,
    types::{InboundKind, UserContentBlock, UserMessageBlock, UserSource},
};
use crate::scheduler::{
    SCHEDULE_REQUEST_MUTATION_LIMIT, ScheduleAdmission, ScheduleBinding, ScheduleCommand,
    ScheduleCommandAuthority, ScheduleCommandResult, ScheduleId, ScheduleMutation,
    ScheduleRequestBudget, ScheduleSnapshot, ScheduleState, ScheduleSummary, ScheduleTarget,
    ScheduleTiming, ScheduleTriggerId, ScheduleTriggerReceipt, ScheduledTargetReceipt,
};
use chrono::{DateTime, Duration, Utc};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use sha2::{Digest, Sha256};

#[cfg(test)]
mod tests;

#[allow(clippy::needless_pass_by_value)] // Owned map_err adapter.
fn sql(error: rusqlite::Error) -> ConversationStoreError {
    storage(format!("Scheduler: {error}"))
}
pub(super) fn create_schema(connection: &Connection) -> Result<(), ConversationStoreError> {
    connection.execute_batch("CREATE TABLE schedule_control(id INTEGER PRIMARY KEY CHECK(id=1), next_ordinal INTEGER NOT NULL, generation INTEGER NOT NULL);
        INSERT INTO schedule_control VALUES(1,0,0);
        CREATE TABLE schedules(ordinal INTEGER PRIMARY KEY, revision INTEGER NOT NULL, state TEXT NOT NULL, next_due TEXT, completed_at TEXT, snapshot_json TEXT NOT NULL);
        CREATE INDEX schedules_due ON schedules(state,next_due,ordinal);
        CREATE INDEX schedules_completed ON schedules(state,completed_at,ordinal);
        CREATE TABLE schedule_commands(scope_id TEXT NOT NULL,command_id TEXT NOT NULL,request_attempt_id TEXT,digest TEXT NOT NULL,result_json TEXT NOT NULL,PRIMARY KEY(scope_id,command_id),CHECK((scope_id='runtime' AND request_attempt_id IS NULL) OR (scope_id GLOB 'human:*' AND request_attempt_id IS NOT NULL)));
        CREATE INDEX pending_scheduler_correlation ON pending_inbound(correlation) WHERE correlation GLOB 'scheduler:*';").map_err(sql)
}
pub(super) fn verify_schema(connection: &Connection) -> Result<(), ConversationStoreError> {
    for (table, expected) in [
        (
            "schedule_control",
            &["id", "next_ordinal", "generation"][..],
        ),
        (
            "schedules",
            &[
                "ordinal",
                "revision",
                "state",
                "next_due",
                "completed_at",
                "snapshot_json",
            ][..],
        ),
        (
            "schedule_commands",
            &[
                "scope_id",
                "command_id",
                "request_attempt_id",
                "digest",
                "result_json",
            ][..],
        ),
    ] {
        let columns = connection
            .prepare(&format!("PRAGMA table_info({table})"))
            .map_err(sql)?
            .query_map([], |row| row.get::<_, String>(1))
            .map_err(sql)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(sql)?;
        if columns != expected {
            return Err(ConversationStoreError::IncompatibleSchema(format!(
                "invalid {table} columns"
            )));
        }
    }
    for (table, column) in [("schedule_control", "id"), ("schedules", "ordinal")] {
        let keys = connection
            .prepare("SELECT name FROM pragma_table_info(?1) WHERE pk>0 ORDER BY pk")
            .map_err(sql)?
            .query_map([table], |row| row.get::<_, String>(0))
            .map_err(sql)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(sql)?;
        if keys != [column] {
            return Err(ConversationStoreError::IncompatibleSchema(format!(
                "invalid {table} primary key"
            )));
        }
    }
    super::schema::shape::verify_unique_columns(
        connection,
        "schedule_commands",
        &["scope_id", "command_id"],
    )?;
    for (index, expected) in [
        ("schedules_due", &["state", "next_due", "ordinal"][..]),
        (
            "schedules_completed",
            &["state", "completed_at", "ordinal"][..],
        ),
        ("pending_scheduler_correlation", &["correlation"][..]),
    ] {
        let columns = connection
            .prepare(&format!("PRAGMA index_info({index})"))
            .map_err(sql)?
            .query_map([], |row| row.get::<_, String>(2))
            .map_err(sql)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(sql)?;
        if columns != expected {
            return Err(ConversationStoreError::IncompatibleSchema(format!(
                "invalid {index} index"
            )));
        }
    }
    let partial: String = connection
        .query_row(
            "SELECT sql FROM sqlite_master WHERE name='pending_scheduler_correlation'",
            [],
            |row| row.get(0),
        )
        .map_err(sql)?;
    if !partial.ends_with("WHERE correlation GLOB 'scheduler:*'") {
        return Err(ConversationStoreError::IncompatibleSchema(
            "invalid Scheduler pending predicate".into(),
        ));
    }
    Ok(())
}
fn invalid(reason: impl Into<String>) -> ScheduleCommandResult {
    ScheduleCommandResult::Invalid {
        reason: reason.into(),
    }
}
fn bounded(title: &str, text: &str) -> bool {
    title.len() <= 480
        && !title.trim().is_empty()
        && title.chars().count() <= 120
        && text.len() <= 16 * 1024
        && !text.trim().is_empty()
}

fn state_text(state: ScheduleState) -> &'static str {
    match state {
        ScheduleState::Enabled => "enabled",
        ScheduleState::Disabled => "disabled",
        ScheduleState::Completed => "completed",
    }
}
fn stamp(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(chrono::SecondsFormat::Nanos, true)
}
fn increment(value: u64) -> Result<u64, ConversationStoreError> {
    value
        .checked_add(1)
        .filter(|v| i64::try_from(*v).is_ok())
        .ok_or(ConversationStoreError::SequenceExhausted)
}
pub(super) fn advance_generation(tx: &Transaction<'_>) -> Result<(), ConversationStoreError> {
    let old: u64 = tx
        .query_row(
            "SELECT generation FROM schedule_control WHERE id=1",
            [],
            |row| row.get(0),
        )
        .map_err(sql)?;
    tx.execute(
        "UPDATE schedule_control SET generation=?1 WHERE id=1",
        [increment(old)?],
    )
    .map_err(sql)?;
    Ok(())
}
fn read(
    connection: &Connection,
    ordinal: u64,
) -> Result<Option<ScheduleSnapshot>, ConversationStoreError> {
    if ordinal > i64::MAX as u64 {
        return Ok(None);
    }
    let json: Option<String> = connection
        .query_row(
            "SELECT snapshot_json FROM schedules WHERE ordinal=?1",
            [ordinal],
            |row| row.get(0),
        )
        .optional()
        .map_err(sql)?;
    json.map(|json| decode(&json, "schedule")).transpose()
}
fn save(tx: &Transaction<'_>, snapshot: &ScheduleSnapshot) -> Result<(), ConversationStoreError> {
    tx.execute("INSERT INTO schedules(ordinal,revision,state,next_due,completed_at,snapshot_json) VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(ordinal) DO UPDATE SET revision=excluded.revision,state=excluded.state,next_due=excluded.next_due,completed_at=excluded.completed_at,snapshot_json=excluded.snapshot_json",
        params![snapshot.id.ordinal,snapshot.revision,state_text(snapshot.state),snapshot.next_due_utc.map(stamp),snapshot.completed_at.map(stamp),encode(snapshot,"schedule")?]).map_err(sql)?;
    Ok(())
}
pub(super) fn prune(
    tx: &Transaction<'_>,
    now: DateTime<Utc>,
) -> Result<(), ConversationStoreError> {
    let cutoff = now
        .checked_sub_signed(Duration::days(30))
        .unwrap_or(DateTime::<Utc>::MIN_UTC);
    tx.execute("DELETE FROM schedules WHERE state='completed' AND (completed_at<?1 OR ordinal IN (SELECT ordinal FROM schedules WHERE state='completed' ORDER BY completed_at DESC,ordinal DESC LIMIT -1 OFFSET 200))",[stamp(cutoff)]).map_err(sql)?;
    let expired = "SELECT start_id FROM scheduled_workflow_starts INDEXED BY scheduled_workflow_completed WHERE state='settled' AND contract_json IS NOT NULL AND (settled_at<?1 OR start_id IN (SELECT start_id FROM scheduled_workflow_starts INDEXED BY scheduled_workflow_completed WHERE state='settled' AND contract_json IS NOT NULL ORDER BY settled_at DESC,start_id DESC LIMIT -1 OFFSET 200))";
    tx.execute(
        &format!("DELETE FROM scheduled_workflow_effects WHERE start_id IN ({expired})"),
        [stamp(cutoff)],
    )
    .map_err(sql)?;
    tx.execute(&format!("UPDATE scheduled_workflow_starts SET contract_json=NULL,args_json=NULL,result_json=NULL WHERE start_id IN ({expired}) AND (contract_json IS NOT NULL OR args_json IS NOT NULL OR result_json IS NOT NULL)"),[stamp(cutoff)]).map_err(sql)?;
    Ok(())
}

fn command_scope(authority: &ScheduleCommandAuthority) -> Option<(String, Option<&str>)> {
    match authority {
        ScheduleCommandAuthority::RuntimeControl => Some(("runtime".to_owned(), None)),
        ScheduleCommandAuthority::HumanRequest {
            message_id,
            original_attempt_id,
        } => {
            let message = message_id.as_str();
            let attempt = original_attempt_id.as_str();
            (!message.is_empty()
                && !attempt.is_empty()
                && message.len() <= 512
                && attempt.len() <= 512)
                .then(|| (format!("human:{message}"), Some(attempt)))
        }
    }
}

fn command_input_error(
    conversation_id: &crate::runtime::identity::ConversationId,
    binding: &ScheduleBinding,
    command: &ScheduleCommand,
) -> Option<&'static str> {
    if binding.conversation_id != *conversation_id
        || command.command_id.is_empty()
        || command.command_id.len() > 128
    {
        return Some("invalid command identity or destination");
    }
    if let ScheduleMutation::Create { title, text, .. }
    | ScheduleMutation::Update { title, text, .. } = &command.mutation
        && !bounded(title, text)
    {
        return Some("invalid title or reminder bounds");
    }
    if let ScheduleMutation::CreateWorkflow {
        title, arguments, ..
    }
    | ScheduleMutation::UpdateWorkflow {
        title, arguments, ..
    } = &command.mutation
        && (!bounded(title, "workflow")
            || serde_json::to_vec(arguments).map_or(true, |json| json.len() > 16384))
    {
        return Some("invalid Workflow title or argument bounds");
    }
    let timing = match &command.mutation {
        ScheduleMutation::Create { timing, .. }
        | ScheduleMutation::CreateWorkflow { timing, .. } => Some(timing),
        ScheduleMutation::Update { timing, .. }
        | ScheduleMutation::UpdateWorkflow { timing, .. } => timing.as_ref(),
        _ => None,
    };
    if timing.is_some_and(|timing| !timing.input_bounded()) {
        return Some("Cron input exceeds native bounds");
    }
    None
}

impl ScheduleStore for SqliteConversationStore {
    fn schedule_model_command(
        &self,
        binding: &ScheduleBinding,
        command: &ScheduleCommand,
        clock: &dyn crate::runtime::types::RuntimeClock,
        authority: &ScheduleCommandAuthority,
        contract: Option<&crate::runtime::workflow::scheduled::PinnedScheduledWorkflow>,
        cancellation: &crate::runtime::cancellation::ExecutionCancellation,
    ) -> Result<ScheduleCommandResult, ConversationStoreError> {
        if !matches!(authority, ScheduleCommandAuthority::HumanRequest { .. }) {
            return Ok(invalid("model Schedule control has no Human request"));
        }
        self.schedule_command_impl(
            binding,
            command,
            clock,
            authority,
            contract,
            Some(cancellation),
        )
    }
    fn schedule_command_receipt(
        &self,
        binding: &ScheduleBinding,
        command: &ScheduleCommand,
        authority: &ScheduleCommandAuthority,
    ) -> Result<Option<ScheduleCommandResult>, ConversationStoreError> {
        if let Some(reason) = command_input_error(&self.conversation_id, binding, command) {
            return Ok(Some(invalid(reason)));
        }
        let Some((scope, attempt)) = command_scope(authority) else {
            return Ok(Some(invalid("invalid native request identity")));
        };
        let digest = format!(
            "{:x}",
            Sha256::digest(encode(&(binding, &command.mutation), "schedule command")?.as_bytes())
        );
        let saved:Option<(String,String,Option<String>)>=self.lock()?.query_row("SELECT digest,result_json,request_attempt_id FROM schedule_commands WHERE scope_id=?1 AND command_id=?2",params![scope,command.command_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional().map_err(sql)?;
        saved
            .map(|(previous, json, original)| {
                if previous == digest && original.as_deref() == attempt {
                    decode(&json, "schedule command receipt")
                } else {
                    Ok(invalid(
                        "command identity conflicts with its committed input or request binding",
                    ))
                }
            })
            .transpose()
    }

    fn schedule_command(
        &self,
        binding: &ScheduleBinding,
        command: &ScheduleCommand,
        clock: &dyn crate::runtime::types::RuntimeClock,
        authority: &ScheduleCommandAuthority,
    ) -> Result<ScheduleCommandResult, ConversationStoreError> {
        self.schedule_command_impl(binding, command, clock, authority, None, None)
    }
    fn schedule_workflow_command(
        &self,
        binding: &ScheduleBinding,
        command: &ScheduleCommand,
        clock: &dyn crate::runtime::types::RuntimeClock,
        authority: &ScheduleCommandAuthority,
        contract: &crate::runtime::workflow::scheduled::PinnedScheduledWorkflow,
    ) -> Result<ScheduleCommandResult, ConversationStoreError> {
        self.schedule_command_impl(binding, command, clock, authority, Some(contract), None)
    }
    fn schedule_get(
        &self,
        id: &ScheduleId,
    ) -> Result<Option<ScheduleSnapshot>, ConversationStoreError> {
        if id.conversation_id != self.conversation_id {
            return Ok(None);
        }
        read(&*self.lock()?, id.ordinal)
    }
    fn schedule_list(
        &self,
        after_ordinal: u64,
        limit: u32,
    ) -> Result<Vec<ScheduleSnapshot>, ConversationStoreError> {
        if !(1..=100).contains(&limit) || after_ordinal > i64::MAX as u64 {
            return Err(ConversationStoreError::ResourceBoundExceeded(
                "schedule page must contain 1–100 rows".into(),
            ));
        }
        let connection = self.lock()?;
        connection
            .prepare(
                "SELECT snapshot_json FROM schedules WHERE ordinal>?1 ORDER BY ordinal LIMIT ?2",
            )
            .map_err(sql)?
            .query_map(params![after_ordinal, limit], |row| row.get::<_, String>(0))
            .map_err(sql)?
            .map(|row| decode(&row.map_err(sql)?, "schedule"))
            .collect()
    }
    fn schedule_due_page(
        &self,
        now: DateTime<Utc>,
        after: Option<&crate::scheduler::ScheduleDueCursor>,
    ) -> Result<crate::scheduler::ScheduleDuePage, ConversationStoreError> {
        let mut connection = self.lock()?;
        let tx = connection.transaction().map_err(sql)?;
        let generation = tx
            .query_row(
                "SELECT generation FROM schedule_control WHERE id=1",
                [],
                |row| row.get(0),
            )
            .map_err(sql)?;
        let after_time = after
            .map(|cursor| stamp(cursor.due_utc))
            .unwrap_or_default();
        let after_ordinal = after.map_or(0, |cursor| cursor.ordinal);
        let mut statement=tx.prepare("SELECT ordinal,revision,next_due FROM schedules WHERE state='enabled' AND next_due<=?1 AND (next_due,ordinal)>(?2,?3) ORDER BY next_due,ordinal LIMIT 64").map_err(sql)?;
        let candidates = statement
            .query_map(params![stamp(now), after_time, after_ordinal], |row| {
                Ok((
                    row.get::<_, u64>(0)?,
                    row.get::<_, u64>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .map_err(sql)?
            .map(|row| {
                let (ordinal, revision, due) = row.map_err(sql)?;
                Ok(crate::scheduler::ScheduleCandidate {
                    id: ScheduleId {
                        conversation_id: self.conversation_id.clone(),
                        ordinal,
                    },
                    revision,
                    due_utc: DateTime::parse_from_rfc3339(&due)
                        .map_err(|e| storage(e.to_string()))?
                        .with_timezone(&Utc),
                })
            })
            .collect::<Result<Vec<_>, ConversationStoreError>>()?;
        let next = (candidates.len() == 64).then(|| {
            let last = candidates.last().expect("full due page");
            crate::scheduler::ScheduleDueCursor {
                due_utc: last.due_utc,
                ordinal: last.id.ordinal,
            }
        });
        Ok(crate::scheduler::ScheduleDuePage {
            generation,
            candidates,
            next,
        })
    }
    fn prune_schedules(
        &self,
        clock: &dyn crate::runtime::types::RuntimeClock,
    ) -> Result<(usize, u64), ConversationStoreError> {
        let mut connection = self.lock()?;
        let tx = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql)?;
        let before = tx.total_changes();
        prune(&tx, clock.now())?;
        let removed = usize::try_from(tx.total_changes() - before)
            .map_err(|_| storage("prune count overflow"))?;
        if removed > 0 {
            advance_generation(&tx)?;
        }
        let generation = tx
            .query_row(
                "SELECT generation FROM schedule_control WHERE id=1",
                [],
                |row| row.get(0),
            )
            .map_err(sql)?;
        tx.commit().map_err(sql)?;
        Ok((removed, generation))
    }
    #[allow(clippy::too_many_lines)] // Keep the complete native transaction and its commit frontier together.
    fn schedule_summary(&self) -> Result<ScheduleSummary, ConversationStoreError> {
        let mut connection = self.lock()?;
        let tx = connection.transaction().map_err(sql)?;
        let generation = tx
            .query_row(
                "SELECT generation FROM schedule_control WHERE id=1",
                [],
                |row| row.get(0),
            )
            .map_err(sql)?;
        let due:Option<String>=tx.query_row("SELECT next_due FROM schedules WHERE state='enabled' ORDER BY next_due,ordinal LIMIT 1",[],|row|row.get(0)).optional().map_err(sql)?.flatten();
        let enabled = tx
            .query_row(
                "SELECT COUNT(*) FROM schedules WHERE state='enabled'",
                [],
                |row| row.get(0),
            )
            .map_err(sql)?;
        let oldest:Option<String>=tx.query_row("SELECT MIN(completed_at) FROM (SELECT completed_at FROM (SELECT completed_at FROM schedules WHERE state='completed' ORDER BY completed_at,ordinal LIMIT 1) UNION ALL SELECT settled_at FROM (SELECT settled_at FROM scheduled_workflow_starts INDEXED BY scheduled_workflow_completed WHERE state='settled' AND contract_json IS NOT NULL ORDER BY settled_at,start_id LIMIT 1))",[],|row|row.get(0)).optional().map_err(sql)?.flatten();
        let oldest_completed_utc = oldest
            .map(|v| {
                DateTime::parse_from_rfc3339(&v)
                    .map(|v| v.with_timezone(&Utc))
                    .map_err(|e| storage(e.to_string()))
            })
            .transpose()?;
        let mut pending = Vec::new();
        let mut statement=tx.prepare("SELECT p.correlation,p.message_json FROM pending_inbound p JOIN inbound_correlation c ON c.correlation=p.correlation AND c.message_id=p.message_id AND c.sequence=p.sequence WHERE p.correlation GLOB 'scheduler:*' ORDER BY p.correlation LIMIT 256").map_err(sql)?;
        let rows = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(sql)?;
        for row in rows {
            let (correlation, json) = row.map_err(sql)?;
            let mut parts = correlation.splitn(4, ':');
            if parts.next() != Some("scheduler") {
                continue;
            }
            let ordinal = parts
                .next()
                .and_then(|s| s.parse().ok())
                .ok_or_else(|| storage("invalid scheduled ordinal"))?;
            let admission_revision = parts
                .next()
                .and_then(|s| s.parse().ok())
                .ok_or_else(|| storage("invalid scheduled revision"))?;
            let occurrence_utc = DateTime::parse_from_rfc3339(
                parts
                    .next()
                    .ok_or_else(|| storage("missing scheduled time"))?,
            )
            .map_err(|e| storage(e.to_string()))?
            .with_timezone(&Utc);
            let trigger = ScheduleTriggerId {
                schedule_id: ScheduleId {
                    conversation_id: self.conversation_id.clone(),
                    ordinal,
                },
                admission_revision,
                occurrence_utc,
            };
            let message: UserMessageBlock = decode(&json, "scheduled pending")?;
            if message.id != trigger.message_id()
                || message.source != UserSource::Runtime
                || message.kind != InboundKind::Message
                || message.timestamp != Some(occurrence_utc)
            {
                return Err(storage("invalid scheduled native provenance"));
            }
            pending.push(trigger);
        }
        let mut ready = pending.clone();
        let mut workflow_rows=tx.prepare("SELECT start_id FROM scheduled_workflow_starts WHERE state IN ('accepted','claimed') ORDER BY accepted_at,start_id LIMIT 256").map_err(sql)?;
        let workflow_rows = workflow_rows
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(sql)?;
        for row in workflow_rows {
            let id =
                crate::runtime::workflow::scheduled::lifecycle::ScheduledWorkflowStartId::try_from(
                    row.map_err(sql)?,
                )
                .map_err(storage)?;
            let head = super::scheduled_workflow::head(&tx, &id)?
                .ok_or_else(|| storage("scheduled Workflow disappeared within read transaction"))?;
            if head.binding.conversation_id != self.conversation_id {
                return Err(storage("foreign Workflow trigger"));
            }
            if matches!(
                head.state,
                crate::runtime::workflow::scheduled::lifecycle::ScheduledWorkflowState::Accepted { .. }
            ) {
                ready.push(head.trigger_id.clone());
            }
            pending.push(head.trigger_id);
        }
        if pending.len() > 256 {
            return Err(storage("outstanding Scheduler target cap exceeded"));
        }
        Ok(ScheduleSummary {
            generation,
            next_due_utc: due
                .map(|v| {
                    DateTime::parse_from_rfc3339(&v)
                        .map(|v| v.with_timezone(&Utc))
                        .map_err(|e| storage(e.to_string()))
                })
                .transpose()?,
            enabled,
            oldest_completed_utc,
            pending,
            ready,
        })
    }
    #[allow(clippy::too_many_lines)] // Keep the complete native transaction and its commit frontier together.
    fn admit_scheduled_target(
        &self,
        id: &ScheduleId,
        expected_revision: u64,
        clock: &dyn crate::runtime::types::RuntimeClock,
    ) -> Result<ScheduleAdmission, ConversationStoreError> {
        if id.conversation_id != self.conversation_id {
            return Ok(ScheduleAdmission::NotFound);
        }
        let mut connection = self.lock()?;
        let tx = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql)?;
        let Some(mut snapshot) = read(&tx, id.ordinal)? else {
            return Ok(ScheduleAdmission::NotFound);
        };
        if snapshot.revision != expected_revision {
            return Ok(ScheduleAdmission::AlreadyAdvanced { current: snapshot });
        }
        let now = clock.now();
        let Some(due) = snapshot
            .next_due_utc
            .filter(|due| *due <= now && snapshot.state == ScheduleState::Enabled)
        else {
            return Ok(ScheduleAdmission::NotDue);
        };
        let prefix = format!("scheduler:{}:*", id.ordinal);
        let outstanding:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM pending_inbound WHERE correlation GLOB 'scheduler:*' AND correlation GLOB ?1) OR EXISTS(SELECT 1 FROM scheduled_workflow_starts WHERE schedule_ordinal=?2 AND state IN ('accepted','claimed'))",params![prefix,id.ordinal],|row|row.get(0)).map_err(sql)?;
        if outstanding {
            return Ok(ScheduleAdmission::WaitingForTarget);
        }
        let count:u64=tx.query_row("SELECT (SELECT COUNT(*) FROM (SELECT 1 FROM pending_inbound WHERE correlation GLOB 'scheduler:*' LIMIT 256)) + (SELECT COUNT(*) FROM (SELECT 1 FROM scheduled_workflow_starts WHERE state IN ('accepted','claimed') LIMIT 256))",[],|row|row.get(0)).map_err(sql)?;
        if count >= 256 {
            return Ok(ScheduleAdmission::SchedulerCapacity);
        }
        let (occurrence_utc, next) = match snapshot.spec.occurrence(due, now) {
            Ok(occurrence) => occurrence,
            Err(reason) => {
                snapshot.state = ScheduleState::Disabled;
                snapshot.revision = increment(snapshot.revision)?;
                snapshot.last_failure = Some(reason.chars().take(256).collect());
                save(&tx, &snapshot)?;
                advance_generation(&tx)?;
                tx.commit().map_err(sql)?;
                return Ok(ScheduleAdmission::Disabled { current: snapshot });
            }
        };
        let trigger_id = ScheduleTriggerId {
            schedule_id: id.clone(),
            admission_revision: expected_revision,
            occurrence_utc,
        };
        let (inbound, workflow) = match &snapshot.target {
            ScheduleTarget::ConversationInbound { text } => {
                let inbound = super::inbox::accept_scheduled_inbound_tx(
                    self,
                    &tx,
                    InboundDraft {
                        message_id: Some(trigger_id.message_id()),
                        source: UserSource::Runtime,
                        kind: InboundKind::Message,
                        content: vec![UserContentBlock::Text(TextBlock {
                            text: format!("Scheduled reminder ({occurrence_utc}):\n{text}"),
                        })],
                        timestamp: occurrence_utc,
                        correlation: Some(trigger_id.correlation()),
                    },
                )?;
                if inbound.retried {
                    return Err(storage(
                        "schedule revision points at an already admitted occurrence",
                    ));
                }
                (Some(inbound), None)
            }
            ScheduleTarget::Workflow { pinned, arguments } => {
                let start = super::scheduled_workflow::accept_tx(
                    &tx,
                    &snapshot.binding,
                    trigger_id.clone(),
                    pinned,
                    arguments,
                    now,
                )?;
                (None, Some(start))
            }
        };
        snapshot.revision = increment(snapshot.revision)?;
        snapshot.next_due_utc = next;
        snapshot.last_failure = None;
        let target = if let Some(inbound) = &inbound {
            ScheduledTargetReceipt::ConversationInbound {
                message_id: inbound.message_id.clone(),
                sequence: inbound.sequence,
            }
        } else {
            let start = workflow.as_ref().expect("closed native target");
            ScheduledTargetReceipt::Workflow {
                start_id: start.id.clone(),
                run_id: start.run_id.clone(),
            }
        };
        snapshot.last_trigger = Some(ScheduleTriggerReceipt {
            trigger_id,
            accepted_at: now,
            target,
        });
        if next.is_none() {
            snapshot.state = ScheduleState::Completed;
            snapshot.completed_at = Some(now);
            snapshot.completion_reason = Some(snapshot.spec.completion_reason());
        }
        save(&tx, &snapshot)?;
        advance_generation(&tx)?;
        prune(&tx, now)?;
        #[cfg(test)]
        if Self::consume(&self.fail_accept_remaining) {
            return Err(storage("fault injected: scheduled admission commit"));
        }
        crate::runtime::process_death::reach("before:admit_scheduled_target");
        tx.commit().map_err(sql)?;
        crate::runtime::process_death::reach("after:admit_scheduled_target");
        if let Some(inbound) = inbound {
            Ok(ScheduleAdmission::Admitted {
                snapshot,
                inbound: Box::new(inbound),
            })
        } else {
            Ok(ScheduleAdmission::WorkflowAdmitted {
                snapshot,
                start: Box::new(workflow.expect("closed native target")),
            })
        }
    }
}

fn create_schedule(
    tx: &Transaction<'_>,
    binding: &ScheduleBinding,
    title: &str,
    target: ScheduleTarget,
    timing: &ScheduleTiming,
    now: DateTime<Utc>,
) -> Result<ScheduleCommandResult, ConversationStoreError> {
    if !bounded(title, "target") {
        return Ok(invalid(
            "title must contain 1–120 characters; text must contain 1–16384 bytes",
        ));
    }
    let (spec, next_due) = match timing.resolve(now) {
        Ok(spec) => spec,
        Err(reason) => return Ok(invalid(reason)),
    };
    let count: u64 = tx
        .query_row(
            "SELECT COUNT(*) FROM schedules WHERE state IN ('enabled','disabled')",
            [],
            |row| row.get(0),
        )
        .map_err(sql)?;
    if count >= 256 {
        return Ok(ScheduleCommandResult::Capacity);
    }
    let old: u64 = tx
        .query_row(
            "SELECT next_ordinal FROM schedule_control WHERE id=1",
            [],
            |row| row.get(0),
        )
        .map_err(sql)?;
    let ordinal = increment(old)?;
    tx.execute(
        "UPDATE schedule_control SET next_ordinal=?1 WHERE id=1",
        [ordinal],
    )
    .map_err(sql)?;
    let snapshot = ScheduleSnapshot {
        id: ScheduleId {
            conversation_id: binding.conversation_id.clone(),
            ordinal,
        },
        binding: binding.clone(),
        revision: 1,
        title: title.to_owned(),
        target,
        next_due_utc: Some(next_due),
        spec,
        state: ScheduleState::Enabled,
        last_trigger: None,
        completed_at: None,
        completion_reason: None,
        last_failure: None,
    };
    save(tx, &snapshot)?;
    Ok(ScheduleCommandResult::Applied {
        receipt: (&snapshot).into(),
    })
}

#[allow(clippy::too_many_lines)] // Keep the complete native transaction and its commit frontier together.
fn apply_command(
    tx: &Transaction<'_>,
    binding: &ScheduleBinding,
    mutation: &ScheduleMutation,
    now: DateTime<Utc>,
    prepared: Option<&crate::runtime::workflow::scheduled::PinnedScheduledWorkflow>,
) -> Result<ScheduleCommandResult, ConversationStoreError> {
    if let ScheduleMutation::Create {
        title,
        text,
        timing,
    } = mutation
    {
        return create_schedule(
            tx,
            binding,
            title,
            ScheduleTarget::ConversationInbound { text: text.clone() },
            timing,
            now,
        );
    }
    if let ScheduleMutation::CreateWorkflow {
        title,
        workflow_id,
        arguments,
        timing,
    } = mutation
    {
        let Some(pinned) = prepared
            .filter(|pin| pin.binding == *binding && pin.definition.workflow_id == *workflow_id)
        else {
            return Ok(invalid("Workflow target requires native preparation"));
        };
        return create_schedule(
            tx,
            binding,
            title,
            ScheduleTarget::Workflow {
                pinned: Box::new(pinned.clone()),
                arguments: arguments.clone(),
            },
            timing,
            now,
        );
    }
    let (id, expected_revision) = match mutation {
        ScheduleMutation::Update {
            id,
            expected_revision,
            ..
        }
        | ScheduleMutation::UpdateWorkflow {
            id,
            expected_revision,
            ..
        }
        | ScheduleMutation::SetEnabled {
            id,
            expected_revision,
            ..
        }
        | ScheduleMutation::Delete {
            id,
            expected_revision,
        } => (id, *expected_revision),

        ScheduleMutation::Create { .. } | ScheduleMutation::CreateWorkflow { .. } => unreachable!(),
    };
    if id.conversation_id != binding.conversation_id {
        return Ok(invalid("schedule belongs to another conversation"));
    }
    let Some(mut snapshot) = read(tx, id.ordinal)? else {
        return Ok(ScheduleCommandResult::NotFound);
    };
    if snapshot.binding != *binding {
        return Ok(invalid("schedule belongs to another Session/node"));
    }
    if snapshot.revision != expected_revision {
        return Ok(ScheduleCommandResult::Conflict {
            current: Box::new(snapshot),
        });
    }
    if let ScheduleMutation::Delete { .. } = mutation {
        tx.execute("DELETE FROM schedules WHERE ordinal=?1", [id.ordinal])
            .map_err(sql)?;
        return Ok(ScheduleCommandResult::Deleted {
            id: id.clone(),
            revision: increment(snapshot.revision)?,
            request_budget: None,
        });
    }
    if snapshot.state == ScheduleState::Completed {
        return Ok(invalid(
            "completed schedules are terminal; create a new schedule",
        ));
    }
    let old = snapshot.clone();
    match mutation {
        ScheduleMutation::Update {
            title,
            text,
            timing,
            ..
        } => {
            if !bounded(title, text) {
                return Ok(invalid("invalid title or reminder bounds"));
            }
            snapshot.title.clone_from(title);
            if !matches!(snapshot.target, ScheduleTarget::ConversationInbound { .. }) {
                return Ok(invalid("Workflow target needs an explicit Workflow update"));
            }
            snapshot.target = ScheduleTarget::ConversationInbound { text: text.clone() };
            if let Some(timing) = timing {
                let (spec, next_due) = match timing.resolve(now) {
                    Ok(spec) => spec,
                    Err(reason) => return Ok(invalid(reason)),
                };
                snapshot.next_due_utc = Some(next_due);
                snapshot.spec = spec;
                snapshot.last_failure = None;
            }
        }
        ScheduleMutation::UpdateWorkflow {
            title,
            workflow_id,
            arguments,
            timing,
            ..
        } => {
            let Some(pinned) = prepared.filter(|pin| {
                pin.binding == *binding && pin.definition.workflow_id == *workflow_id
            }) else {
                return Ok(invalid("Workflow update requires native preparation"));
            };
            snapshot.title.clone_from(title);
            snapshot.target = ScheduleTarget::Workflow {
                pinned: Box::new(pinned.clone()),
                arguments: arguments.clone(),
            };
            if let Some(timing) = timing {
                let (spec, next_due) = match timing.resolve(now) {
                    Ok(value) => value,
                    Err(reason) => return Ok(invalid(reason)),
                };
                snapshot.spec = spec;
                snapshot.next_due_utc = Some(next_due);
                snapshot.last_failure = None;
            }
        }
        ScheduleMutation::SetEnabled { enabled, .. } => {
            snapshot.state = if *enabled {
                ScheduleState::Enabled
            } else {
                ScheduleState::Disabled
            }
        }
        _ => unreachable!(),
    }
    if snapshot != old {
        snapshot.revision = increment(snapshot.revision)?;
        save(tx, &snapshot)?;
    }
    Ok(ScheduleCommandResult::Applied {
        receipt: (&snapshot).into(),
    })
}

impl SqliteConversationStore {
    #[allow(clippy::too_many_lines)] // Keep the complete native transaction and its commit frontier together.
    fn schedule_command_impl(
        &self,
        binding: &ScheduleBinding,
        command: &ScheduleCommand,
        clock: &dyn crate::runtime::types::RuntimeClock,
        authority: &crate::scheduler::ScheduleCommandAuthority,
        prepared: Option<&crate::runtime::workflow::scheduled::PinnedScheduledWorkflow>,
        cancellation: Option<&crate::runtime::cancellation::ExecutionCancellation>,
    ) -> Result<ScheduleCommandResult, ConversationStoreError> {
        if let Some(reason) = command_input_error(&self.conversation_id, binding, command) {
            return Ok(invalid(reason));
        }
        let digest = format!(
            "{:x}",
            Sha256::digest(encode(&(binding, &command.mutation), "schedule command")?.as_bytes())
        );
        let mut connection = self.lock()?;
        let tx = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql)?;
        let Some((scope, attempt)) = command_scope(authority) else {
            return Ok(invalid("request identity exceeds native bounds"));
        };
        let previous:Option<(String,String,Option<String>)>=tx.query_row("SELECT digest,result_json,request_attempt_id FROM schedule_commands WHERE scope_id=?1 AND command_id=?2",params![scope,command.command_id],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?))).optional().map_err(sql)?;
        if let Some((saved, json, saved_attempt)) = previous {
            return if saved == digest && saved_attempt.as_deref() == attempt {
                decode(&json, "schedule command receipt")
            } else {
                Ok(invalid(
                    "command identity conflicts with its committed input or request binding",
                ))
            };
        }
        let used = if let Some(attempt) = attempt {
            let original: Option<String> = tx
                .query_row(
                    "SELECT request_attempt_id FROM schedule_commands WHERE scope_id=?1 LIMIT 1",
                    [&scope],
                    |row| row.get(0),
                )
                .optional()
                .map_err(sql)?;
            if original
                .as_deref()
                .is_some_and(|original| original != attempt)
            {
                return Ok(invalid("Human request original Attempt binding conflicts"));
            }
            let count: u64 = tx
                .query_row(
                    "SELECT COUNT(*) FROM schedule_commands WHERE scope_id=?1",
                    [&scope],
                    |row| row.get(0),
                )
                .map_err(sql)?;
            u8::try_from(count)
                .ok()
                .filter(|count| *count <= SCHEDULE_REQUEST_MUTATION_LIMIT)
                .ok_or_else(|| storage("invalid native request receipt count"))?
        } else {
            0
        };
        if cancellation
            .is_some_and(crate::runtime::cancellation::ExecutionCancellation::is_cancelled)
        {
            return Err(ConversationStoreError::OperationCancelled);
        }
        let now = clock.now();
        let before = tx.total_changes();
        let mut result = apply_command(&tx, binding, &command.mutation, now, prepared)?;
        let changed = tx.total_changes() != before;
        if changed && attempt.is_some() && used >= SCHEDULE_REQUEST_MUTATION_LIMIT {
            return Ok(ScheduleCommandResult::MutationBudgetExhausted);
        }
        if let ScheduleCommandAuthority::HumanRequest {
            message_id,
            original_attempt_id,
        } = authority
        {
            let budget = ScheduleRequestBudget {
                human_message_id: message_id.clone(),
                original_attempt_id: original_attempt_id.clone(),
                mutations_used: used + u8::from(changed),
                mutation_limit: SCHEDULE_REQUEST_MUTATION_LIMIT,
            };
            match &mut result {
                ScheduleCommandResult::Applied { receipt } => receipt.request_budget = Some(budget),
                ScheduleCommandResult::Deleted { request_budget, .. } => {
                    *request_budget = Some(budget);
                }
                _ => {}
            }
        }
        if cancellation
            .is_some_and(crate::runtime::cancellation::ExecutionCancellation::is_cancelled)
        {
            return Err(ConversationStoreError::OperationCancelled);
        }
        if changed {
            advance_generation(&tx)?;
            prune(&tx, now)?;
            tx.execute(
                "INSERT INTO schedule_commands(scope_id,command_id,request_attempt_id,digest,result_json) VALUES(?1,?2,?3,?4,?5)",
                params![
                    scope,
                    command.command_id,
                    attempt,
                    digest,
                    encode(&result, "schedule command receipt")?
                ],
            )
            .map_err(sql)?;
            tx.commit().map_err(sql)?;
        }
        Ok(result)
    }
}
