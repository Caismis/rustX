//! One immutable outcome body, with finite byte ranges for its allowlisted fields.
use super::{
    Connection, ConversationStoreError, ObservationValueChunk, OptionalExtension, RequestId,
    params, sqlite_error, started, storage,
};
use crate::durable::observation::RequestObservationOutcome;
use crate::durable::sqlite::statements::FixedStatements;
use crate::observation::content::{CONTENT_READ_MAX_BYTES, CONTENT_SAFE_INTEGER_MAX};
use crate::observation::trace::frozen::{BorrowedFailure, RequestOutcomeSource};
use serde::{Deserialize, Serialize};
use std::io::Write;

#[cfg(test)]
mod tests;

#[derive(Clone, Copy, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Span {
    start: u64,
    end: u64,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredOutcome {
    header: RequestObservationOutcome,
    // failure, failure.message.text, usage, generation, all relative to the root.
    fields: [Span; 4],
}

fn encode_value<T: Serialize + schemars::JsonSchema>(
    writer: &mut crate::durable::sqlite::content_encoding::SegmentWriter<'_>,
    value: &T,
) -> Result<Span, ConversationStoreError> {
    let start = writer.position();
    crate::observation::structural_json::to_writer(&mut *writer, value)
        .map_err(|error| storage(error.to_string()))?;
    Ok(Span {
        start,
        end: writer.position(),
    })
}

fn capture(
    writer: &mut crate::durable::sqlite::content_encoding::SegmentWriter<'_>,
    source: &RequestOutcomeSource<'_>,
) -> Result<[Span; 4], ConversationStoreError> {
    // The outcome's fixed schema has three fields. Capture their positions as
    // they are written, without parsing the root or serializing children again.
    let mut fields = [Span::default(); 4];
    macro_rules! literal {
        ($bytes:literal) => {
            writer
                .write_all($bytes)
                .map_err(|error| storage(error.to_string()))?
        };
    }
    literal!(b"{\"usage\":");
    fields[2] = encode_value(writer, source.usage)?;
    literal!(b",\"failure\":");
    let start = writer.position();
    if let Some(failure) = &source.failure {
        literal!(b"{\"kind\":");
        encode_value(writer, failure.kind())?;
        literal!(b",\"message\":{\"text\":");
        fields[1] = encode_value(writer, &failure.message())?;
        literal!(b",\"truncated\":false}}");
    } else {
        literal!(b"null");
    }
    fields[0] = Span {
        start,
        end: writer.position(),
    };
    if source.failure.is_none() {
        fields[1] = fields[0];
    }
    literal!(b",\"generation\":");
    fields[3] = encode_value(writer, &source.generation)?;
    literal!(b"}");
    Ok(fields)
}

pub(in crate::durable::sqlite) fn terminal(
    connection: &Connection,
    event: &crate::events::types::RuntimeEventEnvelope,
) -> Result<(), ConversationStoreError> {
    use crate::events::types::RuntimeEvent;
    let (RuntimeEvent::ModelRequestCompleted { request_id, .. }
    | RuntimeEvent::ModelRequestFailed { request_id, .. }) = &event.event
    else {
        return Ok(());
    };
    // Missing request capture must not reject the native terminal or invent evidence.
    let retained: bool = connection
        .query_row_cached(
            "SELECT EXISTS(SELECT 1 FROM request_snapshots WHERE request_id=?1)",
            [request_id.as_str()],
            |row| row.get(0),
        )
        .map_err(|error| sqlite_error(&error, error.to_string()))?;
    if !retained {
        return Ok(());
    }
    let source_sequence = started(connection, request_id, event.sequence)?;
    let source_key = source_sequence.to_string();
    let source = crate::observation::trace::frozen_request_outcome(&event.event)
        .expect("matched native request terminal");
    let mut writer = crate::durable::sqlite::content_encoding::SegmentWriter::new(
        connection,
        (
            "trace_terminal",
            &source_key,
            0,
            crate::observation::content::ContentRepresentation::CanonicalJsonV1,
        ),
        1,
        false,
    );
    let fields = capture(&mut writer, &source)?;
    let encoded_bytes = writer.finish()?;
    let failure_deferred = source
        .failure
        .as_ref()
        .is_some_and(|value| value.message().chars().nth(256).is_some());
    let stored = StoredOutcome {
        header: RequestObservationOutcome {
            terminal_sequence: event.sequence,
            event_id: (event.event_id.as_str().len() <= 512).then(|| event.event_id.clone()),
            usage: source.usage.clone(),
            failure: source.failure.as_ref().map(BorrowedFailure::header),
            generation: source.generation,
            failure_deferred,
            encoded_bytes,
        },
        fields,
    };
    let bytes = crate::observation::bounded::bounded_bytes(&stored, 16_384)
        .ok_or_else(|| storage("native request outcome header exceeds bound"))?;
    let changed=connection.execute_cached("UPDATE request_snapshots SET observation_terminal_sequence=?2,observation_terminal_header=?3 WHERE request_id=?1 AND observation_terminal_sequence IS NULL",params![request_id.as_str(),event.sequence,bytes]).map_err(|error|sqlite_error(&error,error.to_string()))?;
    if changed != 1 {
        return Err(storage(
            "native request terminal header owner missing or duplicated",
        ));
    }
    Ok(())
}

type TerminalRow = (Option<u64>, Option<u64>, Option<Vec<u8>>);

fn load(
    connection: &Connection,
    source_sequence: u64,
    through: u64,
) -> Result<Option<StoredOutcome>, ConversationStoreError> {
    let row:Option<TerminalRow>=connection.query_row("SELECT observation_terminal_sequence,completed_sequence,CASE WHEN typeof(observation_terminal_header)='blob' AND length(observation_terminal_header)<=16384 THEN observation_terminal_header END FROM request_snapshots INDEXED BY request_snapshots_started_idx WHERE started_sequence=?1",[source_sequence],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?))).optional().map_err(|error|sqlite_error(&error, error.to_string()))?;
    let (terminal, completed, bytes) =
        row.ok_or_else(|| storage("original Request snapshot source absent"))?;
    let Some(terminal) = terminal.filter(|sequence| *sequence <= through) else {
        return Ok(None);
    };
    let bytes = bytes.ok_or_else(|| storage("native Request outcome header unavailable"))?;
    let stored: StoredOutcome = serde_json::from_slice(&bytes)
        .map_err(|_| storage("native Request outcome header corrupt"))?;
    if stored.header.terminal_sequence != terminal
        || (stored.header.failure.is_none() && completed != Some(terminal))
        || stored.header.encoded_bytes > CONTENT_SAFE_INTEGER_MAX
        || stored
            .fields
            .iter()
            .any(|span| span.start >= span.end || span.end > stored.header.encoded_bytes)
        || stored.fields[2].end > stored.fields[0].start
        || stored.fields[0].end > stored.fields[3].start
        || stored.fields[1].start < stored.fields[0].start
        || stored.fields[1].end > stored.fields[0].end
    {
        return Err(storage("native Request outcome field geometry corrupt"));
    }
    Ok(Some(stored))
}

pub(in crate::durable::sqlite) fn outcome_at(
    connection: &Connection,
    source_sequence: u64,
    through: u64,
) -> Result<Option<RequestObservationOutcome>, ConversationStoreError> {
    Ok(load(connection, source_sequence, through)?.map(|stored| stored.header))
}

pub(in crate::durable::sqlite) fn outcome_read_at(
    connection: &Connection,
    source_sequence: u64,
    through: u64,
    path: &[String],
    offset: u64,
    max_bytes: u32,
) -> Result<ObservationValueChunk, ConversationStoreError> {
    let stored = load(connection, source_sequence, through)?.ok_or_else(|| {
        ConversationStoreError::InvalidReference(
            "request terminal source is outside native cut".into(),
        )
    })?;
    let field = match path
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        [] => Span {
            start: 0,
            end: stored.header.encoded_bytes,
        },
        ["failure"] => stored.fields[0],
        ["failure", "message", "text"] => stored.fields[1],
        ["usage"] => stored.fields[2],
        ["generation"] => stored.fields[3],
        _ => {
            return Err(ConversationStoreError::InvalidReference(
                "request terminal field is not allowlisted".into(),
            ));
        }
    };
    let readable_end = field.end - field.start;
    if max_bytes == 0 || max_bytes > CONTENT_READ_MAX_BYTES || offset > readable_end {
        return Err(ConversationStoreError::InvalidContentRange);
    }
    let end = offset
        .checked_add(u64::from(max_bytes))
        .filter(|end| *end <= CONTENT_SAFE_INTEGER_MAX)
        .ok_or(ConversationStoreError::InvalidContentRange)?
        .min(readable_end);
    let key = source_sequence.to_string();
    // Authenticate current root availability and length even for an empty field read.
    let length: Option<u64> = connection.query_row_cached(
        "SELECT readable_end FROM content_frontiers WHERE source_kind='trace_terminal' AND source_id=?1 AND block_index=0 AND representation='j' AND revision=1",
        [&key], |row| row.get(0),
    ).optional().map_err(|error|sqlite_error(&error,error.to_string()))?;
    if length != Some(stored.header.encoded_bytes) {
        return Err(storage("native Request outcome body unavailable"));
    }
    let bytes = crate::durable::sqlite::content::range_bytes(
        connection,
        (
            "trace_terminal",
            &key,
            0,
            crate::observation::content::ContentRepresentation::CanonicalJsonV1,
        ),
        field.start + offset,
        field.start + end,
    )?;
    Ok(ObservationValueChunk {
        bytes,
        end_offset: end,
        readable_end,
        next_offset: (end < readable_end).then_some(end),
    })
}

pub(in crate::durable::sqlite) fn outcome(
    connection: &Connection,
    request: &RequestId,
    through: u64,
) -> Result<Option<RequestObservationOutcome>, ConversationStoreError> {
    outcome_at(connection, started(connection, request, through)?, through)
}

pub(in crate::durable::sqlite) fn outcome_read(
    connection: &Connection,
    request: &RequestId,
    through: u64,
    path: &[String],
    offset: u64,
    max_bytes: u32,
) -> Result<ObservationValueChunk, ConversationStoreError> {
    outcome_read_at(
        connection,
        started(connection, request, through)?,
        through,
        path,
        offset,
        max_bytes,
    )
}
