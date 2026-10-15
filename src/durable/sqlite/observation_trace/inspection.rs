//! Optional original-request and adoption relationships; never execution authority.
use super::{
    Connection, ConversationStoreError, ObservationReadCut, OptionalExtension, TraceFactData,
    TraceFieldKind, TraceFieldSource, header, params, storage,
};
use crate::durable::observation::TraceAdoptionPage;
use crate::events::types::{RuntimeEventEnvelope, ToolObservationOccurrence};

pub(in crate::durable::sqlite) fn tool_identity_digest(
    id: &crate::runtime::identity::ToolId,
) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut digest = Sha256::new();
    digest.update(b"rustx/trace/original-tool-id/v1\0");
    digest.update((id.as_str().len() as u64).to_be_bytes());
    digest.update(id.as_str().as_bytes());
    digest.finalize().into()
}

pub(in crate::durable::sqlite) fn index_catalog(
    connection: &Connection,
    start: u64,
    snapshot: &crate::model::RequestSnapshot,
) -> Result<(), ConversationStoreError> {
    connection.execute(
        "INSERT INTO request_trace_catalogs(source_sequence,provisional_message_id,tool_count) VALUES(?1,?2,?3)",
        params![start,snapshot.provisional_message_id.as_str(),snapshot.tool_definitions.len() as u64],
    ).map_err(|error|storage(error.to_string()))?;
    let mut insert = connection.prepare_cached(
        "INSERT INTO request_trace_definitions(source_sequence,ordinal,tool_identity_digest,tool_id) VALUES(?1,?2,?3,?4)",
    ).map_err(|error|storage(error.to_string()))?;
    for (ordinal, definition) in snapshot.tool_definitions.iter().enumerate() {
        insert
            .execute(params![
                start,
                ordinal as u64,
                tool_identity_digest(&definition.id).as_slice(),
                definition.id.as_str()
            ])
            .map_err(|error| storage(error.to_string()))?;
    }
    Ok(())
}

/// Called under a nested optional savepoint after native Assistant C/J assignment.
/// Exact original provisional identity and native owner select a unique request;
/// neither request recency nor a wire call/name participates in this relationship.
pub(in crate::durable::sqlite) fn capture_definitions(
    connection: &Connection,
    event: &RuntimeEventEnvelope,
    position: u64,
    message: &crate::runtime::identity::MessageId,
) -> Result<(), ConversationStoreError> {
    let mut candidates = connection.prepare_cached(
        "SELECT c.source_sequence,c.tool_count FROM request_trace_catalogs c INDEXED BY request_trace_catalog_identity_idx JOIN events e ON e.sequence=c.source_sequence WHERE c.provisional_message_id=?1 AND e.attempt_id IS ?2 AND e.turn_id IS ?3 AND c.source_sequence<?4 ORDER BY c.source_sequence LIMIT 2",
    ).map_err(|error|storage(error.to_string()))?;
    let mut rows = candidates
        .query(params![
            message.as_str(),
            event
                .attempt_id
                .as_ref()
                .map(crate::runtime::identity::AttemptId::as_str),
            event
                .turn_id
                .as_ref()
                .map(crate::runtime::identity::TurnId::as_str),
            event.sequence
        ])
        .map_err(|error| storage(error.to_string()))?;
    let row = rows
        .next()
        .map_err(|error| storage(error.to_string()))?
        .ok_or_else(|| storage("original Tool definition request relationship unavailable"))?;
    let start: u64 = row.get(0).map_err(|error| storage(error.to_string()))?;
    let count: u64 = row.get(1).map_err(|error| storage(error.to_string()))?;
    if rows
        .next()
        .map_err(|error| storage(error.to_string()))?
        .is_some()
    {
        return Err(storage(
            "original Tool definition request relationship ambiguous",
        ));
    }
    if !super::event::accepted_identity_equal(connection, start, event.sequence, message.as_str())?
    {
        return Err(storage(
            "original Tool definition provisional identity differs",
        ));
    }
    let retained: u64 = connection
        .query_row(
            "SELECT count(*) FROM request_trace_definitions WHERE source_sequence=?1",
            [start],
            |row| row.get(0),
        )
        .map_err(|error| storage(error.to_string()))?;
    if retained != count {
        return Err(storage(
            "original Tool definition catalog evidence incomplete",
        ));
    }
    connection
        .execute(
            "UPDATE request_snapshots SET observation_accepted_ledger=?2 WHERE started_sequence=?1",
            params![start, position],
        )
        .map_err(|error| storage(error.to_string()))?;
    // Native historical_definition uses the first exact ToolId in the frozen Vec.
    // This work belongs to original capture; public reads never compare giant IDs.
    connection.execute(
        "UPDATE canonical_tool_calls SET observation_definition_start=?2,observation_definition_commit=?3,observation_definition_ordinal=(SELECT ordinal FROM request_trace_definitions d INDEXED BY request_trace_definition_identity_idx WHERE d.source_sequence=?2 AND d.tool_id=canonical_tool_calls.tool_id ORDER BY ordinal LIMIT 1),observation_definition_ready=1 WHERE observation_ledger=?1",
        params![position,start,event.sequence],
    ).map_err(|error|storage(error.to_string()))?;
    Ok(())
}

type DefinitionRow = (u64, Option<u64>, bool, Option<u64>, Option<u64>);
pub(in crate::durable::sqlite) fn tool_definition(
    connection: &Connection,
    cut: &ObservationReadCut,
    occurrence: &ToolObservationOccurrence,
) -> Result<Option<TraceFieldSource>, ConversationStoreError> {
    if occurrence.ledger_position == 0 || occurrence.ledger_position > cut.ledger_through {
        return Err(ConversationStoreError::InvalidReference(
            "Tool definition beyond original C cut".into(),
        ));
    }
    let row: Option<DefinitionRow> = connection.query_row(
        "SELECT observation_tool_ordinal,observation_definition_commit,observation_definition_ready,observation_definition_start,observation_definition_ordinal FROM canonical_tool_calls INDEXED BY canonical_tool_calls_observation_idx WHERE observation_ledger=?1 AND block_index=?2",
        params![occurrence.ledger_position,occurrence.block_index.get()],
        |row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?)),
    ).optional().map_err(|error|storage(error.to_string()))?;
    let (ordinal, committed, ready, start, definition) = row.ok_or_else(|| {
        ConversationStoreError::InvalidReference(
            "original Tool definition occurrence absent".into(),
        )
    })?;
    if ordinal != occurrence.tool_ordinal {
        return Err(ConversationStoreError::InvalidReference(
            "Tool definition occurrence differs from original C".into(),
        ));
    }
    let committed = committed
        .filter(|sequence| *sequence <= cut.read.journal)
        .ok_or_else(|| {
            storage("original Tool definition request relationship unavailable at cut")
        })?;
    if !matches!(header(connection,committed,cut.read.journal)?.fact,TraceFactData::AssistantCommit{ledger_position} if ledger_position==occurrence.ledger_position)
    {
        return Err(storage("Tool definition accepted C relationship differs"));
    }
    if !ready {
        return Err(storage("original Tool definition capture unavailable"));
    }
    let start = start
        .filter(|start| *start < committed)
        .ok_or_else(|| storage("original Tool definition request relationship unavailable"))?;
    super::super::observation_request::validate_start(connection, start, cut.read.journal)?;
    let count: u64 = connection
        .query_row(
            "SELECT c.tool_count FROM request_trace_catalogs c JOIN request_snapshots r INDEXED BY request_snapshots_started_idx ON r.started_sequence=c.source_sequence WHERE c.source_sequence=?1 AND r.observation_accepted_ledger=?2",
            params![start,occurrence.ledger_position],
            |row| row.get(0),
        )
        .map_err(|_| storage("original Tool definition catalog unavailable"))?;
    let Some(definition) = definition else {
        // A cleared positive pointer cannot claim native catalog absence.
        // Both the native Tool digest and the indexed original catalog must
        // positively prove no matching definition, without loading either ID.
        let (valid, found): (bool, bool) = connection.query_row(
            "SELECT typeof(c.observation_tool_identity_digest)='blob' AND length(c.observation_tool_identity_digest)=32,EXISTS(SELECT 1 FROM request_trace_definitions d INDEXED BY request_trace_definition_digest_idx WHERE d.source_sequence=?3 AND d.tool_identity_digest=c.observation_tool_identity_digest LIMIT 1) FROM canonical_tool_calls c INDEXED BY canonical_tool_calls_observation_idx WHERE c.observation_ledger=?1 AND c.block_index=?2",
            params![occurrence.ledger_position,occurrence.block_index.get(),start],|row|Ok((row.get(0)?,row.get(1)?)),
        ).map_err(|error|storage(error.to_string()))?;
        if !valid || found {
            return Err(storage(
                "original Tool definition positive pointer unavailable",
            ));
        }
        return Ok(None);
    };
    if definition >= count || definition > 9_007_199_254_740_991 {
        return Err(storage("original Tool definition ordinal differs"));
    }
    let same: bool = connection.query_row(
        "SELECT CASE WHEN typeof(c.observation_tool_identity_digest)='blob' AND length(c.observation_tool_identity_digest)=32 AND typeof(d.tool_identity_digest)='blob' AND length(d.tool_identity_digest)=32 AND c.observation_tool_identity_digest=d.tool_identity_digest AND d.ordinal=(SELECT ordinal FROM request_trace_definitions f INDEXED BY request_trace_definition_digest_idx WHERE f.source_sequence=d.source_sequence AND f.tool_identity_digest=c.observation_tool_identity_digest ORDER BY ordinal LIMIT 1) THEN 1 ELSE 0 END FROM canonical_tool_calls c INDEXED BY canonical_tool_calls_observation_idx JOIN request_trace_definitions d ON d.source_sequence=?3 AND d.ordinal=?4 WHERE c.observation_ledger=?1 AND c.block_index=?2",
        params![occurrence.ledger_position,occurrence.block_index.get(),start,definition],|row|row.get(0),
    ).optional().map_err(|error|storage(error.to_string()))?.unwrap_or(false);
    if !same {
        return Err(storage(
            "original Tool definition identity or first ordinal differs",
        ));
    }
    let path = serde_json::to_string(&["tools".to_owned(), definition.to_string()])
        .map_err(|error| storage(error.to_string()))?;
    let bytes: Option<u64> = connection.query_row(
        "SELECT f.end-f.start FROM request_observation_fields f WHERE f.source_id=?1 AND f.path_json=?2 AND EXISTS(SELECT 1 FROM request_trace_definitions d WHERE d.source_sequence=?3 AND d.ordinal=?4)",
        params![start.to_string(),path,start,definition], |row|row.get(0),
    ).optional().map_err(|error|storage(error.to_string()))?;
    Ok(Some(TraceFieldSource::Value {
        field: TraceFieldKind::Definition,
        domain: format!("trace.records.{start}.detail.request.tools"),
        item_offset: definition,
        path: Vec::new(),
        encoded_bytes: bytes
            .ok_or_else(|| storage("original Tool definition field layout unavailable"))?,
    }))
}

pub(in crate::durable::sqlite) fn adoption_page(
    connection: &Connection,
    cut: &ObservationReadCut,
    sequence: u64,
    offset: u64,
    limit: usize,
) -> Result<TraceAdoptionPage, ConversationStoreError> {
    if offset > 9_007_199_254_740_991 || limit == 0 || limit > 32 {
        return Err(ConversationStoreError::InvalidReference(
            "invalid bounded adoption page".into(),
        ));
    }
    let TraceFactData::Adoption {
        total,
        first_ledger_positions,
        ..
    } = header(connection, sequence, cut.read.journal)?.fact
    else {
        return Err(ConversationStoreError::InvalidReference(
            "Trace source is not an original adoption".into(),
        ));
    };
    if offset > total {
        return Err(ConversationStoreError::InvalidReference(
            "adoption offset exceeds original total".into(),
        ));
    }
    let mut statement=connection.prepare_cached(
        "SELECT m.ordinal,m.ledger_position,CASE WHEN typeof(m.identity_digest)='blob' AND length(m.identity_digest)=32 AND typeof(c.observation_identity_digest)='blob' AND length(c.observation_identity_digest)=32 AND m.identity_digest=c.observation_identity_digest THEN 1 ELSE 0 END FROM trace_adoption_members m JOIN message_ledger c ON c.position=m.ledger_position WHERE m.source_sequence=?1 AND m.ordinal>=?2 AND m.ordinal<?3 ORDER BY m.ordinal LIMIT ?4",
    ).map_err(|error|storage(error.to_string()))?;
    let rows = statement
        .query_map(params![sequence, offset, total, limit], |row| {
            Ok((
                row.get::<_, u64>(0)?,
                row.get::<_, u64>(1)?,
                row.get::<_, bool>(2)?,
            ))
        })
        .map_err(|error| storage(error.to_string()))?;
    let mut items = Vec::with_capacity(limit);
    for row in rows {
        let (ordinal, position, identity) = row.map_err(|error| storage(error.to_string()))?;
        if ordinal != offset + items.len() as u64
            || !identity
            || position > cut.ledger_through
            || usize::try_from(ordinal)
                .ok()
                .and_then(|index| first_ledger_positions.get(index))
                .is_some_and(|original| *original != position)
        {
            return Err(storage("original adoption member evidence differs"));
        }
        items.push(super::canonical(connection, cut, position)?);
    }
    let next = offset + items.len() as u64;
    if next < total && items.len() < limit {
        return Err(storage("original adoption member evidence missing"));
    }
    Ok(TraceAdoptionPage {
        total,
        items,
        next_offset: (next < total).then_some(next),
    })
}
