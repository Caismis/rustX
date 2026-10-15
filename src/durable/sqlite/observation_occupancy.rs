//! Finite native reading evidence; original model fields keep their immutable source.
use super::{Connection, ConversationStoreError, OptionalExtension, params, storage};
use crate::events::types::{RuntimeEvent, RuntimeEventEnvelope};
use crate::runtime_client::occupancy::RuntimeClientContextOccupancy;
#[cfg(test)]
thread_local! {
    static FAIL_COMPACTION_CAPTURE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}
#[cfg(test)]
pub(super) struct CompactionCaptureFault;
#[cfg(test)]
impl Drop for CompactionCaptureFault {
    fn drop(&mut self) {
        FAIL_COMPACTION_CAPTURE.with(|fault| fault.set(false));
    }
}
#[cfg(test)]
pub(super) fn compaction_capture_fault() -> CompactionCaptureFault {
    FAIL_COMPACTION_CAPTURE.with(|fault| assert!(!fault.replace(true)));
    CompactionCaptureFault
}

pub(super) fn witness(
    connection: &Connection,
    event: &RuntimeEventEnvelope,
) -> Option<crate::durable::observation::OccupancyObservation> {
    match &event.event {
        RuntimeEvent::CompactionCompleted { occupancy, .. } => {
            let Ok(compaction) = compaction(connection, event) else {
                // The committing caller owns the fail-closed row marker;
                // missing derived capture cannot refuse this original fact.
                return None;
            };
            let projection_head = super::observation_projection::read(connection, event.sequence)
                .ok()
                .flatten()?;
            if projection_head.activation_id != compaction.activation_id
                || projection_head.checkpoint_id != compaction.checkpoint_id
                || projection_head.origin.completed_journal.0 != event.sequence
            {
                return None;
            }
            Some(crate::durable::observation::OccupancyObservation {
                projection_head,
                compaction,
                value: occupancy.as_ref().map(|value| {
                    RuntimeClientContextOccupancy::from_native(
                        value,
                        Some(format!("context.occupancy.{}.model", event.sequence)),
                    )
                }),
            })
        }
        _ => None,
    }
}

pub(super) fn compaction(
    connection: &Connection,
    event: &RuntimeEventEnvelope,
) -> Result<crate::runtime_client::snapshot::RuntimeClientCompactionView, ConversationStoreError> {
    let RuntimeEvent::CompactionCompleted {
        generation,
        checkpoint_id,
        activation_id,
        tokens_before,
        estimated_tokens_after,
        ..
    } = &event.event
    else {
        return Err(ConversationStoreError::InvalidReference(
            "native event is not a completed compaction".into(),
        ));
    };
    #[cfg(test)]
    if FAIL_COMPACTION_CAPTURE.with(std::cell::Cell::get) {
        return Err(storage("fault injected at derived compaction capture"));
    }
    let _ = connection;
    Ok(
        crate::runtime_client::snapshot::RuntimeClientCompactionView {
            generation: *generation,
            checkpoint_id: checkpoint_id.clone(),
            activation_id: *activation_id,
            tokens_before: *tokens_before,
            estimated_tokens_after: *estimated_tokens_after,
        },
    )
}

#[cfg(test)]
thread_local! { static READ_WORK:std::cell::Cell<(usize,usize)>=const{std::cell::Cell::new((0,0))}; }
#[cfg(test)]
pub(super) fn read_work() -> (usize, usize) {
    READ_WORK.with(std::cell::Cell::get)
}

pub(super) fn event(
    connection: &Connection,
    event: &RuntimeEventEnvelope,
) -> Result<(), ConversationStoreError> {
    let value = match &event.event {
        RuntimeEvent::ModelRequestCompleted {
            request_id,
            usage: Some(usage),
            ..
        }
        | RuntimeEvent::ModelRequestFailed {
            request_id,
            usage: Some(usage),
            ..
        } => super::observation_request_summary::occupancy(
            connection,
            request_id,
            usage,
            event.sequence,
        )?,
        RuntimeEvent::CompactionCompleted { occupancy, .. } => occupancy.as_ref().map(|value| {
            RuntimeClientContextOccupancy::from_native(
                value,
                Some(format!("context.occupancy.{}.model", event.sequence)),
            )
        }),
        _ => return Ok(()),
    };
    let bytes = crate::runtime_client::checkpoint::bounded_bytes(&value, 16_384)
        .ok_or_else(|| storage("native occupancy header exceeds bound"))?;
    connection
        .execute(
            "UPDATE events SET observation_occupancy=?1 WHERE sequence=?2",
            params![bytes, event.sequence],
        )
        .map_err(|error| storage(error.to_string()))?;
    Ok(())
}

pub(super) fn inherited(
    connection: &Connection,
    occupancy: Option<&crate::context::occupancy::ContextOccupancy>,
) -> Result<(), ConversationStoreError> {
    #[derive(serde::Serialize, schemars::JsonSchema)]
    struct Source<'a> {
        occupancy: Option<&'a crate::context::occupancy::ContextOccupancy>,
    }
    let value = occupancy.map(|value| {
        RuntimeClientContextOccupancy::from_native(
            value,
            Some("context.occupancy.inherited.model".into()),
        )
    });
    let bytes = crate::runtime_client::checkpoint::bounded_bytes(&value, 16_384)
        .ok_or_else(|| storage("inherited occupancy header exceeds bound"))?;
    connection
        .execute(
            "UPDATE bootstrap_identity SET occupancy_observation=?1 WHERE id=1",
            [bytes],
        )
        .map_err(|error| storage(error.to_string()))?;
    super::observation_request::index_state(
        connection,
        "observation_context",
        "context/inherited",
        &Source { occupancy },
    )
}

pub(super) fn read(
    connection: &Connection,
    through: u64,
) -> Result<Option<RuntimeClientContextOccupancy>, ConversationStoreError> {
    #[cfg(test)]
    READ_WORK.with(|work| work.set((0, 0)));
    let mut selected: Option<(u64, Option<Vec<u8>>)> = None;
    for kind in [
        "model_request_completed",
        "model_request_failed",
        "compaction_completed",
    ] {
        let row:Option<(u64,Option<Vec<u8>>)>=connection.query_row(
            "SELECT sequence,observation_occupancy FROM events INDEXED BY events_context_reading_idx WHERE json_extract(event_json,'$.event.type')=?1 AND (json_extract(event_json,'$.event.type')='compaction_completed' OR COALESCE(json_type(event_json,'$.event.usage'),'null')!='null')=1 AND sequence<=?2 ORDER BY sequence DESC LIMIT 1",
            params![kind,through],|row|Ok((row.get(0)?,row.get(1)?)),
        ).optional().map_err(|error|storage(error.to_string()))?;
        #[cfg(test)]
        if let Some((_, bytes)) = &row {
            READ_WORK.with(|work| {
                let (rows, total) = work.get();
                work.set((rows + 1, total + bytes.as_ref().map_or(0, Vec::len)));
            });
        }
        if row
            .as_ref()
            .is_some_and(|row| selected.as_ref().is_none_or(|old| row.0 > old.0))
        {
            selected = row;
        }
    }
    let bytes = if let Some((_, bytes)) = selected {
        bytes.ok_or_else(|| storage("native occupancy reading source unavailable"))?
    } else {
        let row: Option<Option<Vec<u8>>> = connection
            .query_row(
                "SELECT occupancy_observation FROM bootstrap_identity WHERE id=1",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| storage(error.to_string()))?;
        let Some(bytes) = row else {
            return Ok(None);
        };
        #[cfg(test)]
        READ_WORK.with(|work| work.set((1, bytes.as_ref().map_or(0, Vec::len))));
        bytes.ok_or_else(|| storage("inherited occupancy reading source unavailable"))?
    };
    serde_json::from_slice(&bytes).map_err(|_| storage("native occupancy header is corrupt"))
}

pub(super) fn model_read(
    connection: &Connection,
    sequence: Option<u64>,
    through: u64,
    offset: u64,
    max_bytes: u32,
) -> Result<crate::durable::observation::ObservationValueChunk, ConversationStoreError> {
    let source = if let Some(sequence) = sequence {
        if sequence == 0 || sequence > through {
            return Err(ConversationStoreError::InvalidContentRange);
        }
        let kind = super::observation_event::kind(connection, sequence, through)?
            .ok_or_else(|| storage("occupancy native kind unavailable"))?;
        if kind != "compaction_completed" {
            return Err(ConversationStoreError::InvalidReference(
                "occupancy model source is not a compaction reading".into(),
            ));
        }
        format!("context/{sequence}")
    } else {
        let exists: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM bootstrap_identity WHERE id=1)",
                [],
                |row| row.get(0),
            )
            .map_err(|error| storage(error.to_string()))?;
        if !exists {
            return Err(ConversationStoreError::InvalidReference(
                "inherited occupancy source unavailable".into(),
            ));
        }
        "context/inherited".into()
    };
    let chunk = super::observation_request::read_from(
        connection,
        "observation_context",
        "state_observation_fields",
        &source,
        &["occupancy".into(), "model".into()],
        offset,
        max_bytes,
    )?;
    Ok(chunk)
}
