//! Immutable Request Snapshot storage, bounded reads, and identity validation.

use super::validate_request_start_metadata;
use crate::durable::ConversationStoreError;
use crate::durable::sqlite::canonical::ledger::load_message;
use crate::durable::sqlite::journal::validate_event_identity;
use crate::durable::sqlite::request::contribution::validate_carryover_snapshot;
use crate::durable::sqlite::{
    decode, manifest, seq_to_i64, sequence_from_i64, sqlite_error, storage,
};
use crate::events::EVENT_SCHEMA_VERSION;
use crate::events::types::{RuntimeEvent, RuntimeEventEnvelope};
use crate::model::snapshot::RequestSnapshot;
use crate::model::types::ModelRequest;
use crate::runtime::AttemptId;
use crate::runtime::TurnId;
use crate::runtime::identity::{ConversationId, RequestId};
use rusqlite::OptionalExtension;
use rusqlite::{Connection, Transaction, params};
use sha2::Digest;
use sha2::Sha256;

#[cfg(test)]
thread_local! {
    pub(in crate::durable::sqlite) static REQUEST_INPUT_RECONSTRUCTIONS: std::cell::Cell<(usize, usize)> = const { std::cell::Cell::new((0, 0)) };
}

#[cfg(test)]
pub(crate) fn take_request_input_reconstructions() -> (usize, usize) {
    REQUEST_INPUT_RECONSTRUCTIONS.with(|count| count.replace((0, 0)))
}

pub(in crate::durable::sqlite) fn reconstruct_snapshot_input(
    connection: &Connection,
    conversation: &ConversationId,
    snapshot: &RequestSnapshot,
) -> Result<ModelRequest, ConversationStoreError> {
    #[cfg(test)]
    REQUEST_INPUT_RECONSTRUCTIONS.with(|count| {
        let (requests, originals) = count.get();
        count.set((requests + 1, originals));
    });
    snapshot
        .reconstruct_with(|source| {
            if &source.conversation_id != conversation {
                return Err(
                    crate::model::snapshot::RequestReconstructionError::Conversation(
                        "frozen source scope mismatch".into(),
                    ),
                );
            }
            #[cfg(test)]
            REQUEST_INPUT_RECONSTRUCTIONS.with(|count| {
                let (requests, originals) = count.get();
                count.set((requests, originals + 1));
            });
            load_message(connection, &source.message_id).map_err(|e| {
                crate::model::snapshot::RequestReconstructionError::Conversation(e.to_string())
            })
        })
        .map_err(|e| ConversationStoreError::InvalidReference(e.to_string()))
}

pub(in crate::durable::sqlite) fn validate_accepted_continuation_source(
    connection: &Connection,
    conversation: &ConversationId,
    snapshot: &RequestSnapshot,
) -> Result<(), ConversationStoreError> {
    let Some(source) = &snapshot.continuation_source else {
        return if snapshot.continuation.is_none() {
            Ok(())
        } else {
            Err(ConversationStoreError::InvalidReference(
                "continuation has no accepted-output source".into(),
            ))
        };
    };
    if snapshot.continuation.is_none()
        || source.request_id == snapshot.request_id
        || &source.assistant.conversation_id != conversation
    {
        return Err(ConversationStoreError::InvalidReference(
            "invalid actual continuation source ownership".into(),
        ));
    }
    let previous = load_request_snapshot(connection, conversation, &source.request_id)?;
    if previous.provisional_message_id != source.assistant.message_id {
        return Err(ConversationStoreError::InvalidReference(
            "continuation source is not the prior request's accepted output".into(),
        ));
    }
    let output = load_message(connection, &source.assistant.message_id)?;
    let input = reconstruct_snapshot_input(connection, conversation, &previous)?;
    let expected =
        crate::runtime::continuation::AcceptedOutputContinuationSource::from_accepted_output(
            previous.request_id,
            conversation,
            &input.messages,
            &output,
        )
        .map_err(ConversationStoreError::InvalidReference)?;
    if source != &expected {
        return Err(ConversationStoreError::InvalidReference(
            "continuation accepted-output or covered-input integrity mismatch".into(),
        ));
    }
    Ok(())
}

pub(in crate::durable::sqlite) fn load_request_snapshot(
    connection: &Connection,
    conversation: &ConversationId,
    request_id: &RequestId,
) -> Result<RequestSnapshot, ConversationStoreError> {
    load_request_snapshot_bounded(connection, conversation, request_id, 64 * 1024 * 1024)
}

pub(in crate::durable::sqlite) fn load_request_snapshot_bounded(
    connection: &Connection,
    conversation: &ConversationId,
    request_id: &RequestId,
    max_bytes: usize,
) -> Result<RequestSnapshot, ConversationStoreError> {
    // The normalized [] cannot exceed the eventual flat input array. Enforce
    // both metadata budgets in SQL before rusqlite allocates its String.
    let metadata_limit = max_bytes.min(64 * 1024 * 1024);
    #[cfg(test)]
    REQUEST_SNAPSHOT_READ_WORK.with(|work| {
        let (queries, bytes) = work.get();
        work.set((queries + 1, bytes));
    });
    let (stored_canonical_frontier, json, started_sequence, digest, native_oversized): (i64, Option<String>, Option<i64>, String, bool) = connection
        .query_row("SELECT canonical_frontier,CASE WHEN octet_length(snapshot_json)<=?2 THEN snapshot_json END,started_sequence,CASE WHEN typeof(manifest_digest)='text' AND octet_length(manifest_digest)=64 THEN manifest_digest ELSE '' END,octet_length(snapshot_json)>67108864 FROM request_snapshots WHERE request_id=?1", params![request_id.as_str(),metadata_limit], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, read_manifest_digest(row, 3)?,row.get(4)?)))
        .optional().map_err(|error| sqlite_error(&error, format!("request snapshot lookup: {error}")))?
        .ok_or_else(|| ConversationStoreError::RequestNotFound(request_id.clone()))?;
    let json = json.ok_or_else(|| {
        if native_oversized {
            ConversationStoreError::InvalidReference(
                "frozen request record exceeds the byte budget".into(),
            )
        } else {
            crate::context::resources::bound(
                "historical frozen request exceeds materialization bound",
            )
        }
    })?;
    #[cfg(test)]
    REQUEST_SNAPSHOT_READ_WORK.with(|work| {
        let (queries, bytes) = work.get();
        work.set((queries, bytes + json.len()));
    });
    // Normalization retained the two-byte empty input array. Charge the
    // actual flat sequence in its place before hydrating any selected leaf.
    let input_budget = max_bytes
        .checked_sub(json.len().saturating_sub(2))
        .ok_or_else(|| {
            crate::context::resources::bound(
                "historical frozen request exceeds materialization bound",
            )
        })?;
    let root = read_stored_snapshot_root(connection, request_id)?;
    if stored_snapshot_digest(&json, root) != digest {
        return Err(ConversationStoreError::InvalidReference(
            "frozen request manifest integrity mismatch".into(),
        ));
    }
    let started_sequence = started_sequence.ok_or_else(|| {
        ConversationStoreError::InvalidReference(format!(
            "request snapshot {request_id} has no durable start sequence"
        ))
    })?;
    let sequence = sequence_from_i64(started_sequence)?;
    let event_json: String = connection
        .query_row(
            "SELECT event_json FROM events WHERE sequence=?1",
            [started_sequence],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| sqlite_error(&error, format!("request start event lookup: {error}")))?
        .ok_or_else(|| {
            ConversationStoreError::InvalidReference(format!(
                "request snapshot {request_id} start event is unavailable"
            ))
        })?;
    let mut snapshot: RequestSnapshot = decode(&json, "request snapshot")?;
    validate_snapshot_identity(&snapshot)?;
    if stored_canonical_frontier != seq_to_i64(snapshot.canonical_frontier.0)? {
        return Err(ConversationStoreError::InvalidReference(format!(
            "request snapshot {request_id} Surface column disagrees with its frozen snapshot"
        )));
    }
    if snapshot.request_id != *request_id {
        return Err(ConversationStoreError::InvalidReference(format!(
            "request snapshot row {request_id} contains a different RequestId"
        )));
    }
    let event: RuntimeEventEnvelope = decode(&event_json, "request start event")?;
    if event.sequence != sequence {
        return Err(ConversationStoreError::InvalidReference(format!(
            "request snapshot {request_id} start event sequence disagrees"
        )));
    }
    if event.conversation_id != *conversation {
        return Err(ConversationStoreError::InvalidReference(format!(
            "request snapshot {request_id} start event belongs to a foreign conversation"
        )));
    }
    validate_request_start_metadata(&snapshot, &event)?;
    if !snapshot.input.is_empty() {
        return Err(ConversationStoreError::InvalidReference(
            "normalized frozen request metadata contains selected input".into(),
        ));
    }
    snapshot.input = manifest::load_bounded(connection, root, input_budget)?;
    Ok(snapshot)
}

// These expression indexes cover the original native Journal JSON, rather
// than optional observation headers. Two starts retain duplicate detection;
// one winner per terminal kind retains earliest native sequence ordering.
pub(in crate::durable::sqlite) const REQUEST_START_PROBE_SQL: &str =
    "SELECT sequence,event_id,schema_version,conversation_id,attempt_id,turn_id,event_json
    FROM events INDEXED BY events_request_id_kind_idx
    WHERE json_extract(event_json,'$.event.request_id')=?1
      AND json_extract(event_json,'$.event.type')='model_request_started'
    ORDER BY sequence LIMIT 2";

pub(in crate::durable::sqlite) const REQUEST_OUTCOME_PROBE_SQL: &str = "SELECT event_json FROM (
    SELECT event_json,sequence FROM (
        SELECT event_json,sequence FROM events INDEXED BY events_request_id_kind_idx
        WHERE json_extract(event_json,'$.event.request_id')=?1
          AND json_extract(event_json,'$.event.type')='model_request_completed'
        ORDER BY sequence LIMIT 1)
    UNION ALL
    SELECT event_json,sequence FROM (
        SELECT event_json,sequence FROM events INDEXED BY events_request_id_kind_idx
        WHERE json_extract(event_json,'$.event.request_id')=?1
          AND json_extract(event_json,'$.event.type')='model_request_failed'
        ORDER BY sequence LIMIT 1))
    ORDER BY sequence LIMIT 1";

#[cfg(test)]
thread_local! {
    pub(in crate::durable::sqlite) static REQUEST_PROBE_WORK: std::cell::Cell<(usize,usize)> = const { std::cell::Cell::new((0,0)) };
}

#[cfg(test)]
pub(in crate::durable::sqlite) fn record_request_probe_work(json: &str) {
    REQUEST_PROBE_WORK.with(|work| {
        let (rows, bytes) = work.get();
        work.set((rows + 1, bytes + json.len()));
    });
}

pub(in crate::durable::sqlite) fn has_request_journal_fact_tx(
    transaction: &Transaction<'_>,
    request_id: &RequestId,
) -> Result<bool, ConversationStoreError> {
    #[cfg(test)]
    let _probe = crate::durable::sqlite::request_probe::activate(
        request_id,
        crate::durable::sqlite::RequestJournalProbe::Orphan,
    );

    transaction
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM events INDEXED BY events_request_id_kind_idx
             WHERE json_extract(event_json, '$.event.request_id')=?1 LIMIT 1)",
            [request_id.as_str()],
            |row| row.get(0),
        )
        .map_err(|error| sqlite_error(&error, format!("request owner orphan probe: {error}")))
}

pub(in crate::durable::sqlite) fn find_request_start_event(
    transaction: &Transaction<'_>,
    request_id: &RequestId,
) -> Result<Option<RuntimeEventEnvelope>, ConversationStoreError> {
    #[cfg(test)]
    let _probe = crate::durable::sqlite::request_probe::activate(
        request_id,
        crate::durable::sqlite::RequestJournalProbe::StartEvent,
    );

    let mut statement = transaction
        .prepare_cached(REQUEST_START_PROBE_SQL)
        .map_err(|error| sqlite_error(&error, format!("request-start event probe: {error}")))?;
    let rows = statement
        .query_map([request_id.as_str()], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, String>(6)?,
            ))
        })
        .map_err(|error| sqlite_error(&error, format!("request-start event query: {error}")))?;
    let mut found = None;
    for row in rows {
        let (sequence, event_id, schema_version, conversation_id, attempt_id, turn_id, json) =
            row.map_err(|error| sqlite_error(&error, format!("request-start event row: {error}")))?;
        #[cfg(test)]
        record_request_probe_work(&json);
        if found.is_some() {
            return Err(ConversationStoreError::InvalidReference(format!(
                "request {request_id} has duplicate request-start facts"
            )));
        }
        let event: RuntimeEventEnvelope = decode(&json, "request-start event")?;
        if !matches!(
            &event.event,
            RuntimeEvent::ModelRequestStarted {
                request_id: candidate,
                ..
            } if candidate == request_id
        ) {
            return Err(ConversationStoreError::InvalidReference(format!(
                "request-start lookup for {request_id} returned a different fact"
            )));
        }
        if event.sequence != sequence_from_i64(sequence)?
            || event.event_id.as_str() != event_id
            || event.schema_version != EVENT_SCHEMA_VERSION
            || i64::from(event.schema_version) != schema_version
            || event.conversation_id.as_str() != conversation_id
            || event.attempt_id.as_ref().map(AttemptId::as_str) != attempt_id.as_deref()
            || event.turn_id.as_ref().map(TurnId::as_str) != turn_id.as_deref()
        {
            return Err(ConversationStoreError::InvalidReference(format!(
                "request-start fact for {request_id} disagrees with its stored envelope"
            )));
        }
        validate_event_identity(&event)?;
        found = Some(event);
    }
    Ok(found)
}

/// Reads one Request Snapshot row through the same durable decoder used by
/// request history. The row's denormalized Surface and sequence columns are
/// checked against the frozen JSON so publication cannot trust a mismatched
/// tuple assembled by a caller.
/// Scalar request identity and configuration, with no selected-input hydration.
/// Full request disclosure must explicitly load the separately bound native root.
#[derive(Debug)]
pub(in crate::durable::sqlite) struct StoredRequestMetadata(
    pub(in crate::durable::sqlite) RequestSnapshot,
);

impl std::ops::Deref for StoredRequestMetadata {
    type Target = RequestSnapshot;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

pub(in crate::durable::sqlite) type RequestSnapshotRow =
    (StoredRequestMetadata, Option<u64>, Option<u64>);

pub(in crate::durable::sqlite) type StartedRequestRow = (StoredRequestMetadata, u64, Option<u64>);

pub(in crate::durable::sqlite) type StoredSnapshotRow =
    (i64, Option<String>, Option<i64>, Option<i64>, String);

#[cfg(test)]
thread_local! {
    pub(in crate::durable::sqlite) static REQUEST_SNAPSHOT_READ_WORK: std::cell::Cell<(usize, usize)> = const { std::cell::Cell::new((0, 0)) };
}

#[cfg(test)]
thread_local! {
    pub(in crate::durable::sqlite) static MANIFEST_DIGEST_READ_WORK: std::cell::Cell<(usize, usize)> = const { std::cell::Cell::new((0, 0)) };
}

// Every caller selects a fixed-size text digest or an empty rejection value.
pub(in crate::durable::sqlite) fn read_manifest_digest(
    row: &rusqlite::Row<'_>,
    index: usize,
) -> rusqlite::Result<String> {
    let digest: String = row.get(index)?;
    #[cfg(test)]
    MANIFEST_DIGEST_READ_WORK.with(|work| {
        let (rows, bytes) = work.get();
        work.set((rows + 1, bytes + digest.len()));
    });
    Ok(digest)
}

pub(in crate::durable::sqlite) fn read_request_snapshot_tx(
    transaction: &Transaction<'_>,
    request_id: &RequestId,
) -> Result<Option<RequestSnapshotRow>, ConversationStoreError> {
    #[cfg(test)]
    REQUEST_SNAPSHOT_READ_WORK.with(|work| {
        let (queries, bytes) = work.get();
        work.set((queries + 1, bytes));
    });
    let row: Option<StoredSnapshotRow> = transaction
        .query_row(
            "SELECT canonical_frontier,CASE WHEN octet_length(snapshot_json)<=67108864 THEN snapshot_json END,started_sequence,completed_sequence,CASE WHEN typeof(manifest_digest)='text' AND octet_length(manifest_digest)=64 THEN manifest_digest ELSE '' END
             FROM request_snapshots WHERE request_id=?1",
            [request_id.as_str()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, read_manifest_digest(row, 4)?)),
        )
        .optional()
        .map_err(|error| sqlite_error(&error, format!("request snapshot transaction lookup: {error}")))?;
    let Some((stored_canonical_frontier, json, started_sequence, completed_sequence, digest)) = row
    else {
        return Ok(None);
    };
    let json = json.ok_or_else(|| {
        ConversationStoreError::InvalidReference(
            "frozen request record exceeds the byte budget".into(),
        )
    })?;
    #[cfg(test)]
    REQUEST_SNAPSHOT_READ_WORK.with(|work| {
        let (queries, bytes) = work.get();
        work.set((queries, bytes + json.len()));
    });
    let root = read_stored_snapshot_root(transaction, request_id)?;
    if stored_snapshot_digest(&json, root) != digest {
        return Err(ConversationStoreError::InvalidReference(
            "frozen request manifest integrity mismatch".into(),
        ));
    }
    let snapshot: RequestSnapshot = decode(&json, "request snapshot")?;
    validate_snapshot_identity(&snapshot)?;
    if !snapshot.input.is_empty() {
        return Err(ConversationStoreError::InvalidReference(
            "normalized frozen request metadata contains selected input".into(),
        ));
    }
    if snapshot.request_id != *request_id {
        return Err(ConversationStoreError::InvalidReference(format!(
            "request snapshot row {request_id} contains a different RequestId"
        )));
    }
    if stored_canonical_frontier != seq_to_i64(snapshot.canonical_frontier.0)? {
        return Err(ConversationStoreError::InvalidReference(format!(
            "request snapshot {request_id} Surface column disagrees with its frozen snapshot"
        )));
    }
    Ok(Some((
        StoredRequestMetadata(snapshot),
        started_sequence.map(sequence_from_i64).transpose()?,
        completed_sequence.map(sequence_from_i64).transpose()?,
    )))
}

/// Proves that a Request Snapshot has exactly one durable request-start fact
/// and that the fact's envelope identifies the same conversation, attempt,
/// turn, request, and model.
pub(in crate::durable::sqlite) fn require_started_request_tx(
    transaction: &Transaction<'_>,
    conversation_id: &ConversationId,
    request_id: &RequestId,
) -> Result<StartedRequestRow, ConversationStoreError> {
    let Some(row) = read_request_snapshot_tx(transaction, request_id)? else {
        return Err(ConversationStoreError::InvalidReference(format!(
            "request snapshot {request_id} does not exist"
        )));
    };
    validate_started_request_row_tx(transaction, conversation_id, request_id, row)
}

pub(in crate::durable::sqlite) fn verified_started_request_tx(
    transaction: &Transaction<'_>,
    conversation_id: &ConversationId,
    request_id: &RequestId,
) -> Result<(RequestSnapshot, RuntimeEventEnvelope), ConversationStoreError> {
    let Some((snapshot, started_sequence, _)) = read_request_snapshot_tx(transaction, request_id)?
    else {
        return Err(ConversationStoreError::InvalidReference(format!(
            "request snapshot {request_id} does not exist"
        )));
    };
    let Some(started_sequence) = started_sequence else {
        return Err(ConversationStoreError::InvalidReference(format!(
            "request snapshot {request_id} has no durable start sequence"
        )));
    };
    let started = find_request_start_event(transaction, request_id)?.ok_or_else(|| {
        ConversationStoreError::InvalidReference(format!(
            "request snapshot {request_id} has no durable request-start fact"
        ))
    })?;
    if started.sequence != started_sequence {
        return Err(ConversationStoreError::InvalidReference(format!(
            "request snapshot {request_id} start sequence disagrees with its request-start fact"
        )));
    }
    if started.conversation_id != *conversation_id {
        return Err(ConversationStoreError::InvalidReference(format!(
            "request-start fact for {request_id} belongs to a foreign conversation"
        )));
    }
    validate_request_start_metadata(&snapshot, &started)?;
    Ok((snapshot.0, started))
}

/// Reuses a row decoded in this same transaction, preserving all original start
/// checks. Callers retain their operation-specific missing-row diagnostic.
pub(in crate::durable::sqlite) fn validate_started_request_row_tx(
    transaction: &Transaction<'_>,
    conversation_id: &ConversationId,
    request_id: &RequestId,
    (snapshot, started_sequence, completed_sequence): RequestSnapshotRow,
) -> Result<StartedRequestRow, ConversationStoreError> {
    let Some(started_sequence) = started_sequence else {
        return Err(ConversationStoreError::InvalidReference(format!(
            "request snapshot {request_id} has no durable start sequence"
        )));
    };
    let Some(started) = find_request_start_event(transaction, request_id)? else {
        return Err(ConversationStoreError::InvalidReference(format!(
            "request snapshot {request_id} has no durable request-start fact"
        )));
    };
    if started.sequence != started_sequence {
        return Err(ConversationStoreError::InvalidReference(format!(
            "request snapshot {request_id} start sequence disagrees with its request-start fact"
        )));
    }
    if started.conversation_id != *conversation_id {
        return Err(ConversationStoreError::InvalidReference(format!(
            "request-start fact for {request_id} belongs to a foreign conversation"
        )));
    }
    validate_request_start_metadata(&snapshot, &started)?;
    Ok((snapshot, started_sequence, completed_sequence))
}

/// Finds any already durable provider terminal for one request. Both success
/// and failure are terminal outcomes: a contradictory second outcome is never
/// accepted, while only a successful outcome establishes P.
pub(in crate::durable::sqlite) fn find_request_outcome_event(
    transaction: &Transaction<'_>,
    request_id: &RequestId,
) -> Result<Option<RuntimeEventEnvelope>, ConversationStoreError> {
    let mut statement = transaction
        .prepare_cached(REQUEST_OUTCOME_PROBE_SQL)
        .map_err(|error| sqlite_error(&error, format!("request outcome probe: {error}")))?;
    let rows = statement
        .query_map([request_id.as_str()], |row| row.get::<_, String>(0))
        .map_err(|error| sqlite_error(&error, format!("request outcome query: {error}")))?;
    for row in rows {
        let json =
            row.map_err(|error| sqlite_error(&error, format!("request outcome row: {error}")))?;
        #[cfg(test)]
        record_request_probe_work(&json);
        let event: RuntimeEventEnvelope = decode(&json, "request outcome")?;
        if matches!(
            &event.event,
            RuntimeEvent::ModelRequestCompleted {
                request_id: candidate,
                ..
            }
                | RuntimeEvent::ModelRequestFailed {
                    request_id: candidate,
                    ..
                } if candidate == request_id
        ) {
            return Ok(Some(event));
        }
    }
    Ok(None)
}

pub(in crate::durable::sqlite) fn validate_snapshot_identity(
    snapshot: &RequestSnapshot,
) -> Result<(), ConversationStoreError> {
    let derived = snapshot.identity.request_id();
    if snapshot.request_id != derived {
        return Err(ConversationStoreError::InvalidReference(format!(
            "Request Snapshot identity {} disagrees with its RequestIdentity-derived id {derived}",
            snapshot.request_id
        )));
    }
    let derived_message = snapshot.identity.provisional_message_id();
    if snapshot.provisional_message_id != derived_message {
        return Err(ConversationStoreError::InvalidReference(format!(
            "Request Snapshot {} provisional message identity disagrees with its RequestIdentity-derived id {derived_message}",
            snapshot.request_id
        )));
    }
    validate_carryover_snapshot(snapshot)?;
    Ok(())
}

/// Binds normalized metadata to the exact immutable selected-input root.
/// This is an internal storage integrity digest, not a replacement for the
/// original flat provider-semantic or canonical-body SHA identities.
pub(in crate::durable::sqlite) fn stored_snapshot_digest(
    json: &str,
    root: Option<manifest::NodeRef>,
) -> String {
    let mut digest = Sha256::new();
    digest.update(b"rustx-normalized-request-v1\0");
    digest.update((json.len() as u64).to_le_bytes());
    digest.update(json.as_bytes());
    if let Some(root) = root {
        digest.update([1]);
        digest.update(root.id.to_le_bytes());
        digest.update(root.digest);
        digest.update(root.count.to_le_bytes());
        digest.update([root.height]);
    } else {
        digest.update([0]);
    }
    format!("{:x}", digest.finalize())
}

pub(in crate::durable::sqlite) fn read_stored_snapshot_root(
    connection: &Connection,
    request: &RequestId,
) -> Result<Option<manifest::NodeRef>, ConversationStoreError> {
    type RootRow = (Option<u64>, Option<Vec<u8>>, u64, Option<u8>, bool);
    let row: Option<RootRow> = connection.prepare_cached(
        "SELECT r.input_root,CASE WHEN length(r.input_root_digest)=32 THEN r.input_root_digest END,r.input_count,n.height,r.input_root_digest IS NULL
         FROM request_snapshots r LEFT JOIN native_input_nodes n ON n.id=r.input_root WHERE r.request_id=?1",
    ).map_err(|error| sqlite_error(&error, format!("request input root lookup: {error}")))?
        .query_row([request.as_str()], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)))
        .optional().map_err(|error| sqlite_error(&error, format!("request input root row: {error}")))?;
    let row = row.ok_or_else(|| ConversationStoreError::RequestNotFound(request.clone()))?;
    match row {
        (None, None, 0, None, true) => Ok(None),
        (Some(id), Some(digest), count, Some(height), false)
            if id > 0
                && (1..=manifest::MAX_ITEMS).contains(&count)
                && (1..=manifest::MAX_HEIGHT).contains(&height) =>
        {
            let root = manifest::NodeRef {
                id,
                digest: digest.try_into().map_err(|_| {
                    ConversationStoreError::InvalidReference(
                        "native request input root digest".into(),
                    )
                })?,
                count,
                height,
            };
            // One root and its direct children establish the finite native
            // geometry. No selected leaf body is needed for scalar callers.
            manifest::node(connection, &root)?;
            Ok(Some(root))
        }
        _ => Err(ConversationStoreError::InvalidReference(
            "native request input root has an invalid closed shape".into(),
        )),
    }
}

pub(in crate::durable::sqlite) fn encode_snapshot(
    snapshot: &RequestSnapshot,
) -> Result<String, ConversationStoreError> {
    struct SnapshotBytes(Vec<u8>);
    impl std::io::Write for SnapshotBytes {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if self
                .0
                .len()
                .checked_add(bytes.len())
                .is_none_or(|size| size > 64 * 1024 * 1024)
            {
                return Err(std::io::Error::other(
                    "frozen request exceeds storage byte budget",
                ));
            }
            self.0.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    // The selected sequence has its own immutable native root. Cloning this
    // handle is O(1); metadata never serializes or copies historical leaves.
    let mut metadata = snapshot.clone();
    metadata.input.clear();
    let mut writer = SnapshotBytes(Vec::new());
    serde_json::to_writer(&mut writer, &metadata)
        .map_err(|error| storage(format!("request snapshot encoding: {error}")))?;
    String::from_utf8(writer.0).map_err(|error| storage(error.to_string()))
}
