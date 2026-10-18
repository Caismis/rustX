//! Immutable request relationships, separate from own lifecycle facts.
use super::{
    Connection, ConversationStoreError, ObservationReadCut, OptionalExtension,
    TraceCanonicalSummary, TraceFactData, TraceFieldKind, TraceFieldSource,
    TraceRequestContextPage, TraceRequestPredecessorHeader, decode, encode, header, params,
    storage, summary,
};
use crate::runtime_client::trace::{TraceContextPresentation, TraceContextSource};

pub(in crate::durable::sqlite) fn predecessor(
    connection: &Connection,
    start: u64,
    through: u64,
) -> Result<Option<TraceRequestPredecessorHeader>, ConversationStoreError> {
    super::super::observation_request::validate_start(connection, start, through)?;
    let previous:Option<u64>=connection.query_row("SELECT sequence FROM events INDEXED BY events_trace_kind_idx WHERE observation_kind='model_request_started' AND sequence<?1 ORDER BY sequence DESC LIMIT 1",[start],|row|row.get(0)).optional().map_err(|error|storage(error.to_string()))?;
    previous
        .map(|previous| {
            let own = header(connection, previous, through)?;
            let TraceFactData::RequestStart { request_id, .. } = own.fact else {
                return Err(storage("original predecessor is not a request"));
            };
            let source = own.fields.into_iter().find(|source| {
                matches!(
                    source,
                    TraceFieldSource::Value {
                        field: TraceFieldKind::RequestId,
                        ..
                    }
                )
            });
            Ok(TraceRequestPredecessorHeader {
                start_sequence: previous,
                request_id,
                source,
                summary: summary(connection, previous, through)?,
            })
        })
        .transpose()
}
pub(in crate::durable::sqlite) fn retry_failure(
    connection: &Connection,
    start: u64,
    through: u64,
) -> Result<Option<crate::model::error::ModelErrorKind>, ConversationStoreError> {
    super::super::observation_request::validate_start(connection, start, through)?;
    let (step,retry):(Option<u64>,Option<u32>)=connection.query_row("SELECT observation_step_anchor,observation_retry_number FROM request_snapshots INDEXED BY request_snapshots_started_idx WHERE started_sequence=?1",[start],|row|Ok((row.get(0)?,row.get(1)?))).map_err(|_|storage("original request retry relationship absent"))?;
    let retry = retry.ok_or_else(|| storage("native retry relationship unavailable"))?;
    if retry == 0 {
        return Ok(None);
    }
    let step = step.ok_or_else(|| storage("native retry Step unavailable"))?;
    let missing:bool=connection.query_row("SELECT EXISTS(SELECT 1 FROM events INDEXED BY events_trace_holes_idx WHERE observation_trace_fact_required=1 AND observation_trace_fact IS NULL AND sequence>?1 AND sequence<?2 AND sequence<=?3)",params![step,start,through],|row|row.get(0)).map_err(|error|storage(error.to_string()))?;
    if missing {
        return Err(storage("native retry terminal evidence unavailable"));
    }
    let previous:Option<u64>=connection.query_row("SELECT started_sequence FROM request_snapshots INDEXED BY request_trace_retry_idx WHERE observation_step_anchor=?1 AND observation_retry_number=?2 AND started_sequence<?3",params![step,retry-1,start],|row|row.get(0)).optional().map_err(|error|storage(error.to_string()))?;
    let Some(previous) = previous else {
        return Ok(None);
    };
    let terminal:Option<u64>=connection.query_row("SELECT sequence FROM events INDEXED BY events_trace_owner_idx WHERE observation_trace_owner=?1 AND observation_kind='model_request_failed' AND sequence<?2 AND sequence<=?3 ORDER BY sequence DESC LIMIT 1",params![previous,start,through],|row|row.get(0)).optional().map_err(|error|storage(error.to_string()))?;
    terminal.map_or(Ok(None), |sequence| {
        header(connection, sequence, through).map(|value| match value.fact {
            TraceFactData::Terminal(end) => end.failure_kind,
            _ => None,
        })
    })
}
pub(in crate::durable::sqlite) fn index_context(
    connection: &Connection,
    start: u64,
    snapshot: &crate::model::RequestSnapshot,
    _context: &[crate::message::types::MessageBlock],
) -> Result<(), ConversationStoreError> {
    for (ordinal, id) in snapshot.request_context_ids.iter().enumerate() {
        let contribution = snapshot
            .contributions
            .iter()
            .find(|value| &value.message_id == id)
            .ok_or_else(|| storage("original accepted context owner absent"))?;
        let position: u64 = connection
            .query_row(
                "SELECT position FROM message_ledger WHERE message_id=?1",
                [id.as_str()],
                |row| row.get(0),
            )
            .map_err(|_| storage("original accepted context C absent"))?;
        let bytes: Option<Vec<u8>> = connection
            .query_row(
                "SELECT observation_trace_summary FROM message_ledger WHERE position=?1",
                [position],
                |row| row.get(0),
            )
            .map_err(|error| storage(error.to_string()))?;
        let canonical: TraceCanonicalSummary =
            decode(&bytes.ok_or_else(|| storage("native context presentation unavailable"))?)?;
        let source = match canonical.source {
            Some(crate::message::types::UserSource::Runtime) => TraceContextSource::Runtime,
            Some(crate::message::types::UserSource::Extension { contributor }) => {
                TraceContextSource::CertifiedExtension { contributor }
            }
            _ => return Err(storage("native accepted Context source differs")),
        };
        let context_kind = canonical
            .context_kind
            .ok_or_else(|| storage("native accepted Context kind absent"))?;
        let truncated = canonical.truncated || canonical.attachments.len() > 4;
        let value = TraceContextPresentation {
            message_id: canonical.identity,
            locator: canonical.locator,
            fields: canonical.fields,
            producer: contribution.producer.clone(),
            context_kind,
            source,
            preview: canonical.preview,
            attachments: canonical.attachments.into_iter().take(4).collect(),
            truncated,
        };
        connection.execute("INSERT INTO request_trace_contexts(source_sequence,ordinal,header_json) VALUES(?1,?2,?3)",params![start,ordinal as u64,encode(&value)?]).map_err(|error|storage(error.to_string()))?;
    }
    connection
        .execute(
            "UPDATE request_snapshots SET observation_context_total=?2 WHERE started_sequence=?1",
            params![start, snapshot.request_context_ids.len() as u64],
        )
        .map_err(|error| storage(error.to_string()))?;
    Ok(())
}
pub(in crate::durable::sqlite) fn context(
    connection: &Connection,
    cut: &ObservationReadCut,
    start: u64,
    offset: u64,
    limit: usize,
) -> Result<TraceRequestContextPage, ConversationStoreError> {
    super::super::observation_request::validate_start(connection, start, cut.read.journal)?;
    if offset > 9_007_199_254_740_991 || limit == 0 || limit > 16 {
        return Err(ConversationStoreError::InvalidReference(
            "invalid bounded Context page".into(),
        ));
    }
    let total:Option<u64>=connection.query_row("SELECT observation_context_total FROM request_snapshots INDEXED BY request_snapshots_started_idx WHERE started_sequence=?1",[start],|row|row.get(0)).map_err(|_|storage("original request snapshot absent"))?;
    let total = total.ok_or_else(|| storage("native request Context evidence unavailable"))?;
    let mut statement=connection.prepare_cached("SELECT ordinal,header_json FROM request_trace_contexts WHERE source_sequence=?1 AND ordinal>=?2 AND ordinal<?3 ORDER BY ordinal LIMIT ?4").map_err(|error|storage(error.to_string()))?;
    let rows = statement
        .query_map(params![start, offset, total, limit], |row| {
            Ok((row.get::<_, u64>(0)?, row.get::<_, Vec<u8>>(1)?))
        })
        .map_err(|error| storage(error.to_string()))?;
    let mut items = Vec::with_capacity(limit);
    for row in rows {
        let (ordinal, bytes) = row.map_err(|error| storage(error.to_string()))?;
        if ordinal != offset + items.len() as u64 {
            return Err(storage("native request Context row missing"));
        }
        let value: TraceContextPresentation = decode(&bytes)?;
        let crate::runtime_client::content::CanonicalContentLocator::ByLedgerPosition { position } =
            value.locator
        else {
            return Err(storage("native Context has no numeric C"));
        };
        if position > cut.ledger_through {
            return Err(storage("native Context C exceeds captured cut"));
        }
        items.push(value);
    }
    let next = offset + items.len() as u64;
    if next < total && items.len() < limit {
        return Err(storage("native request Context row missing"));
    }
    Ok(TraceRequestContextPage {
        total,
        items,
        next_offset: (next < total).then_some(next),
        domain: format!("trace.records.{start}.context"),
    })
}
