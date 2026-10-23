//! Schedule CAS and native inbound acceptance share one immediate transaction.
use super::{SqliteConversationStore, decode, encode, storage};
use crate::durable::{ConversationStoreError, InboundDraft, ScheduleStore};
use crate::message::{
    content::TextBlock,
    types::{InboundKind, UserContentBlock, UserMessageBlock, UserSource},
};
use crate::scheduler::{
    ScheduleAdmission, ScheduleBinding, ScheduleCommand, ScheduleCommandResult, ScheduleId,
    ScheduleMutation, ScheduleSnapshot, ScheduleState, ScheduleSummary, ScheduleTiming,
    ScheduleTriggerId, ScheduleTriggerReceipt,
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
        CREATE TABLE schedule_commands(command_id TEXT PRIMARY KEY, digest TEXT NOT NULL, result_json TEXT NOT NULL);
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
            &["command_id", "digest", "result_json"][..],
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
    for (table, column) in [
        ("schedule_control", "id"),
        ("schedules", "ordinal"),
        ("schedule_commands", "command_id"),
    ] {
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
fn advance_generation(tx: &Transaction<'_>) -> Result<(), ConversationStoreError> {
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
fn prune(tx: &Transaction<'_>, now: DateTime<Utc>) -> Result<(), ConversationStoreError> {
    let cutoff = now
        .checked_sub_signed(Duration::days(30))
        .unwrap_or(DateTime::<Utc>::MIN_UTC);
    tx.execute("DELETE FROM schedules WHERE state='completed' AND (completed_at<?1 OR ordinal IN (SELECT ordinal FROM schedules WHERE state='completed' ORDER BY completed_at DESC,ordinal DESC LIMIT -1 OFFSET 200))",[stamp(cutoff)]).map_err(sql)?;
    Ok(())
}

impl ScheduleStore for SqliteConversationStore {
    fn schedule_command(
        &self,
        binding: &ScheduleBinding,
        command: &ScheduleCommand,
        clock: &dyn crate::runtime::types::RuntimeClock,
    ) -> Result<ScheduleCommandResult, ConversationStoreError> {
        if binding.conversation_id != self.conversation_id
            || command.command_id.is_empty()
            || command.command_id.len() > 128
        {
            return Ok(invalid("invalid command identity or destination"));
        }
        if let ScheduleMutation::Create { title, text, .. }
        | ScheduleMutation::Update { title, text, .. } = &command.mutation
            && !bounded(title, text)
        {
            return Ok(invalid("invalid title or reminder bounds"));
        }
        let digest = format!(
            "{:x}",
            Sha256::digest(encode(&(binding, &command.mutation), "schedule command")?.as_bytes())
        );
        let mut connection = self.lock()?;
        let tx = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql)?;
        let previous: Option<(String, String)> = tx
            .query_row(
                "SELECT digest,result_json FROM schedule_commands WHERE command_id=?1",
                [&command.command_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(sql)?;
        if let Some((saved, json)) = previous {
            return if saved == digest {
                decode(&json, "schedule command receipt")
            } else {
                Ok(invalid(
                    "command identity conflicts with its committed input",
                ))
            };
        }
        let now = clock.now();
        let before = tx.total_changes();
        let result = apply_command(&tx, binding, &command.mutation, now)?;
        if tx.total_changes() != before {
            advance_generation(&tx)?;
            prune(&tx, now)?;
            tx.execute(
                "INSERT INTO schedule_commands(command_id,digest,result_json) VALUES(?1,?2,?3)",
                params![
                    command.command_id,
                    digest,
                    encode(&result, "schedule command receipt")?
                ],
            )
            .map_err(sql)?;
            tx.commit().map_err(sql)?;
        }
        Ok(result)
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
            pending,
        })
    }
    fn admit_scheduled_inbound(
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
        let outstanding:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM pending_inbound WHERE correlation GLOB 'scheduler:*' AND correlation GLOB ?1)",[prefix],|row|row.get(0)).map_err(sql)?;
        if outstanding {
            return Ok(ScheduleAdmission::WaitingForTarget);
        }
        let count:u64=tx.query_row("SELECT COUNT(*) FROM (SELECT 1 FROM pending_inbound WHERE correlation GLOB 'scheduler:*' LIMIT 256)",[],|row|row.get(0)).map_err(sql)?;
        if count >= 256 {
            return Ok(ScheduleAdmission::SchedulerCapacity);
        }
        let (occurrence_utc, next) = snapshot.spec.occurrence(due, now).map_err(storage)?;
        let trigger_id = ScheduleTriggerId {
            schedule_id: id.clone(),
            admission_revision: expected_revision,
            occurrence_utc,
        };
        let inbound = super::inbox::accept_scheduled_inbound_tx(
            self,
            &tx,
            InboundDraft {
                message_id: Some(trigger_id.message_id()),
                source: UserSource::Runtime,
                kind: InboundKind::Message,
                content: vec![UserContentBlock::Text(TextBlock {
                    text: format!("Scheduled reminder ({occurrence_utc}):\n{}", snapshot.text),
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
        snapshot.revision = increment(snapshot.revision)?;
        snapshot.next_due_utc = next;
        snapshot.last_trigger = Some(ScheduleTriggerReceipt {
            trigger_id,
            accepted_at: now,
            message_id: inbound.message_id.clone(),
            sequence: inbound.sequence,
        });
        if next.is_none() {
            snapshot.state = ScheduleState::Completed;
            snapshot.completed_at = Some(now);
        }
        save(&tx, &snapshot)?;
        advance_generation(&tx)?;
        prune(&tx, now)?;
        #[cfg(test)]
        if Self::consume(&self.fail_accept_remaining) {
            return Err(storage("fault injected: scheduled admission commit"));
        }
        crate::runtime::process_death::reach("before:admit_scheduled_inbound");
        tx.commit().map_err(sql)?;
        crate::runtime::process_death::reach("after:admit_scheduled_inbound");
        Ok(ScheduleAdmission::Admitted {
            snapshot,
            inbound: Box::new(inbound),
        })
    }
}

fn create_schedule(
    tx: &Transaction<'_>,
    binding: &ScheduleBinding,
    title: &str,
    text: &str,
    timing: &ScheduleTiming,
    now: DateTime<Utc>,
) -> Result<ScheduleCommandResult, ConversationStoreError> {
    if !bounded(title, text) {
        return Ok(invalid(
            "title must contain 1–120 characters; text must contain 1–16384 bytes",
        ));
    }
    let spec = match timing.resolve(now) {
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
        text: text.to_owned(),
        next_due_utc: Some(spec.first()),
        spec,
        state: ScheduleState::Enabled,
        last_trigger: None,
        completed_at: None,
    };
    save(tx, &snapshot)?;
    Ok(ScheduleCommandResult::Applied {
        receipt: (&snapshot).into(),
    })
}

fn apply_command(
    tx: &Transaction<'_>,
    binding: &ScheduleBinding,
    mutation: &ScheduleMutation,
    now: DateTime<Utc>,
) -> Result<ScheduleCommandResult, ConversationStoreError> {
    if let ScheduleMutation::Create {
        title,
        text,
        timing,
    } = mutation
    {
        return create_schedule(tx, binding, title, text, timing, now);
    }
    let (id, expected_revision) = match mutation {
        ScheduleMutation::Update {
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
        ScheduleMutation::Create { .. } => unreachable!(),
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
            snapshot.text.clone_from(text);
            if let Some(timing) = timing {
                let spec = match timing.resolve(now) {
                    Ok(spec) => spec,
                    Err(reason) => return Ok(invalid(reason)),
                };
                snapshot.next_due_utc = Some(spec.first());
                snapshot.spec = spec;
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
