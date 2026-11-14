//! Durable SQLite storage and bounded authorized reads for immutable
//! admission-bound Workflow run definition snapshots (Issue #552).

use base64::Engine as _;
use rusqlite::{Connection, OptionalExtension, Transaction, params};

use super::{ConversationStoreError, seq_to_i64, sequence_from_i64, storage};
use crate::durable::observation::ObservationReadCut;
use crate::runtime::identity::ConversationId;
use crate::runtime::workflow::snapshot::{
    compute_snapshot_digest, encode_snapshot_document, sha256_hex,
};
use crate::runtime::workflow::{
    WORKFLOW_SNAPSHOT_CHUNK_BYTES, WorkflowId, WorkflowRunDefinitionSnapshot, WorkflowRunId,
    WorkflowRunOrigin, WorkflowRunSnapshotChunk, WorkflowRunSnapshotHeader,
};
use crate::workflow_file::{MAX_WORKFLOW_DOCUMENT_BYTES, WorkflowFileDocument};

pub(super) fn create_schema(connection: &Connection) -> Result<(), ConversationStoreError> {
    connection
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS workflow_definition_snapshots (
                snapshot_digest TEXT PRIMARY KEY,
                workflow_id TEXT NOT NULL,
                program_digest TEXT NOT NULL,
                source_revision TEXT NOT NULL,
                has_source_presentation INTEGER NOT NULL CHECK(has_source_presentation IN (0, 1)),
                byte_length INTEGER NOT NULL CHECK(byte_length >= 0),
                snapshot_sha256 TEXT NOT NULL,
                UNIQUE(workflow_id, program_digest, source_revision)
            ) WITHOUT ROWID;
            CREATE TABLE IF NOT EXISTS workflow_snapshot_chunks (
                snapshot_digest TEXT NOT NULL REFERENCES workflow_definition_snapshots(snapshot_digest) ON DELETE CASCADE,
                chunk_index INTEGER NOT NULL CHECK(chunk_index >= 0),
                bytes BLOB NOT NULL CHECK(typeof(bytes) = 'blob' AND length(bytes) <= 65536),
                PRIMARY KEY (snapshot_digest, chunk_index)
            ) WITHOUT ROWID;
            CREATE TABLE IF NOT EXISTS workflow_run_snapshots (
                conversation_id TEXT NOT NULL,
                origin_type TEXT NOT NULL CHECK(origin_type IN ('tool', 'scheduled')),
                origin_id TEXT NOT NULL,
                invocation INTEGER NOT NULL CHECK(invocation >= 0),
                workflow_id TEXT NOT NULL,
                program_digest TEXT NOT NULL,
                source_revision TEXT NOT NULL,
                snapshot_digest TEXT NOT NULL REFERENCES workflow_definition_snapshots(snapshot_digest),
                committed_sequence INTEGER NOT NULL UNIQUE REFERENCES events(sequence) ON DELETE CASCADE,
                PRIMARY KEY (conversation_id, origin_type, origin_id, invocation)
            ) WITHOUT ROWID;",
        )
        .map_err(|error| storage(format!("create workflow snapshot schema: {error}")))?;
    Ok(())
}

pub(in crate::durable::sqlite) fn verify_schema(
    connection: &Connection,
) -> Result<(), ConversationStoreError> {
    let tables: [(&str, &[&str]); 3] = [
        (
            "workflow_definition_snapshots",
            &[
                "snapshot_digest",
                "workflow_id",
                "program_digest",
                "source_revision",
                "has_source_presentation",
                "byte_length",
                "snapshot_sha256",
            ],
        ),
        (
            "workflow_snapshot_chunks",
            &["snapshot_digest", "chunk_index", "bytes"],
        ),
        (
            "workflow_run_snapshots",
            &[
                "conversation_id",
                "origin_type",
                "origin_id",
                "invocation",
                "workflow_id",
                "program_digest",
                "source_revision",
                "snapshot_digest",
                "committed_sequence",
            ],
        ),
    ];
    for (table, expected_columns) in tables {
        let actual = connection
            .prepare(&format!("PRAGMA table_info({table})"))
            .map_err(|error| storage(format!("inspect {table}: {error}")))?
            .query_map([], |row| row.get::<_, String>(1))
            .map_err(|error| storage(format!("query {table}: {error}")))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| storage(format!("read {table}: {error}")))?;
        if actual.iter().map(String::as_str).collect::<Vec<_>>() != expected_columns {
            return Err(ConversationStoreError::IncompatibleSchema(format!(
                "missing or invalid schema for {table}"
            )));
        }
    }
    Ok(())
}

fn origin_parts(origin: &WorkflowRunOrigin) -> (&'static str, &str) {
    match origin {
        WorkflowRunOrigin::Tool { attempt_id } => ("tool", attempt_id.as_str()),
        WorkflowRunOrigin::Scheduled { start_id } => ("scheduled", start_id.as_str()),
    }
}

pub(super) fn publish_in_tx(
    tx: &Transaction<'_>,
    conversation_id: &ConversationId,
    committed_sequence: u64,
    run_id: &WorkflowRunId,
    snapshot: &WorkflowRunDefinitionSnapshot,
) -> Result<(), ConversationStoreError> {
    if run_id.conversation_id != *conversation_id {
        return Err(ConversationStoreError::InvalidReference(
            "workflow run snapshot belongs to another conversation".into(),
        ));
    }
    let (bytes, payload_sha256, snapshot_digest) =
        encode_snapshot_document(snapshot).map_err(ConversationStoreError::InvalidReference)?;
    let byte_length = i64::try_from(bytes.len())
        .map_err(|_| ConversationStoreError::InvalidReference("snapshot size overflow".into()))?;
    let has_source = i64::from(snapshot.has_source_presentation);

    upsert_definition_and_chunks(
        tx,
        snapshot,
        &snapshot_digest,
        &payload_sha256,
        has_source,
        byte_length,
        &bytes,
    )?;

    let (origin_type, origin_id) = origin_parts(&run_id.origin);
    let invocation = i64::try_from(run_id.invocation)
        .map_err(|_| ConversationStoreError::InvalidReference("invocation overflow".into()))?;
    let committed_sequence = seq_to_i64(committed_sequence)?;
    tx.execute(
        "INSERT INTO workflow_run_snapshots (
            conversation_id, origin_type, origin_id, invocation,
            workflow_id, program_digest, source_revision, snapshot_digest, committed_sequence
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            conversation_id.as_str(),
            origin_type,
            origin_id,
            invocation,
            snapshot.workflow_id.as_str(),
            snapshot.program_digest,
            snapshot.source_revision,
            snapshot_digest,
            committed_sequence,
        ],
    )
    .map_err(|error| storage(format!("insert workflow run snapshot binding: {error}")))?;
    Ok(())
}

fn upsert_definition_and_chunks(
    tx: &Transaction<'_>,
    snapshot: &WorkflowRunDefinitionSnapshot,
    snapshot_digest: &str,
    payload_sha256: &str,
    has_source: i64,
    byte_length: i64,
    bytes: &[u8],
) -> Result<(), ConversationStoreError> {
    let existing_def: Option<(String, i64, i64, String)> = tx
        .query_row(
            "SELECT snapshot_digest, has_source_presentation, byte_length, snapshot_sha256
             FROM workflow_definition_snapshots
             WHERE workflow_id=?1 AND program_digest=?2 AND source_revision=?3",
            params![
                snapshot.workflow_id.as_str(),
                snapshot.program_digest,
                snapshot.source_revision
            ],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()
        .map_err(|error| storage(format!("lookup workflow snapshot definition: {error}")))?;

    if let Some((existing_digest, existing_pres, existing_len, existing_sha)) = existing_def {
        if existing_digest != snapshot_digest
            || existing_pres != has_source
            || existing_len != byte_length
            || existing_sha != payload_sha256
        {
            return Err(ConversationStoreError::TerminalViolation(
                "conflicting workflow definition snapshot for same (workflow_id, program_digest, source_revision)"
                    .into(),
            ));
        }
        let expected_chunks = i64::try_from(bytes.len().div_ceil(WORKFLOW_SNAPSHOT_CHUNK_BYTES))
            .map_err(|_| storage("workflow snapshot chunk count overflow"))?;
        let chunk_count: i64 = tx
            .query_row(
                "SELECT count(*) FROM workflow_snapshot_chunks WHERE snapshot_digest=?1",
                params![existing_digest],
                |row| row.get(0),
            )
            .map_err(|error| storage(format!("verify existing snapshot chunks: {error}")))?;
        if chunk_count != expected_chunks {
            return Err(storage("corrupt existing workflow snapshot chunks"));
        }
        return Ok(());
    }

    tx.execute(
        "INSERT INTO workflow_definition_snapshots (
            snapshot_digest, workflow_id, program_digest, source_revision,
            has_source_presentation, byte_length, snapshot_sha256
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            snapshot_digest,
            snapshot.workflow_id.as_str(),
            snapshot.program_digest,
            snapshot.source_revision,
            has_source,
            byte_length,
            payload_sha256
        ],
    )
    .map_err(|error| storage(format!("insert workflow definition snapshot: {error}")))?;

    let mut insert_chunk = tx
        .prepare_cached(
            "INSERT INTO workflow_snapshot_chunks (snapshot_digest, chunk_index, bytes)
             VALUES (?1, ?2, ?3)",
        )
        .map_err(|error| storage(format!("prepare snapshot chunk insert: {error}")))?;
    for (index, chunk) in bytes.chunks(WORKFLOW_SNAPSHOT_CHUNK_BYTES).enumerate() {
        let chunk_index =
            i64::try_from(index).map_err(|_| storage("workflow snapshot chunk overflow"))?;
        insert_chunk
            .execute(params![snapshot_digest, chunk_index, chunk])
            .map_err(|error| storage(format!("insert workflow snapshot chunk: {error}")))?;
    }
    Ok(())
}

pub(super) fn verify_idempotent_binding(
    tx: &Transaction<'_>,
    conversation_id: &ConversationId,
    committed_sequence: u64,
    run_id: &WorkflowRunId,
    snapshot: &WorkflowRunDefinitionSnapshot,
) -> Result<(), ConversationStoreError> {
    let (_, _, snapshot_digest) =
        encode_snapshot_document(snapshot).map_err(ConversationStoreError::InvalidReference)?;
    let (origin_type, origin_id) = origin_parts(&run_id.origin);
    let invocation = i64::try_from(run_id.invocation)
        .map_err(|_| ConversationStoreError::InvalidReference("invocation overflow".into()))?;
    let bound: Option<(String, i64)> = tx
        .query_row(
            "SELECT snapshot_digest, committed_sequence
             FROM workflow_run_snapshots
             WHERE conversation_id=?1 AND origin_type=?2 AND origin_id=?3 AND invocation=?4",
            params![conversation_id.as_str(), origin_type, origin_id, invocation],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|error| storage(format!("lookup idempotent run snapshot: {error}")))?;
    let Some((bound_digest, bound_seq)) = bound else {
        return Err(ConversationStoreError::TerminalViolation(
            "existing WorkflowStarted event lacks its atomic snapshot binding".into(),
        ));
    };
    let bound_seq = sequence_from_i64(bound_seq)?;
    if bound_digest != snapshot_digest || bound_seq != committed_sequence {
        return Err(ConversationStoreError::TerminalViolation(
            "WorkflowStarted retry conflicts with its bound definition snapshot".into(),
        ));
    }
    Ok(())
}

struct StoredSnapshotMeta {
    header: WorkflowRunSnapshotHeader,
}

fn load_header(
    connection: &Connection,
    conversation_id: &ConversationId,
    cut: &ObservationReadCut,
    run_id: &WorkflowRunId,
) -> Result<Option<StoredSnapshotMeta>, ConversationStoreError> {
    if run_id.conversation_id != *conversation_id {
        return Err(ConversationStoreError::InvalidReference(
            "workflow run belongs to another conversation".into(),
        ));
    }
    let (origin_type, origin_id) = origin_parts(&run_id.origin);
    let invocation = i64::try_from(run_id.invocation)
        .map_err(|_| ConversationStoreError::InvalidReference("invocation overflow".into()))?;
    let binding: Option<(String, String, String, String, i64)> = connection
        .query_row(
            "SELECT workflow_id, program_digest, source_revision, snapshot_digest, committed_sequence
             FROM workflow_run_snapshots
             WHERE conversation_id=?1 AND origin_type=?2 AND origin_id=?3 AND invocation=?4",
            params![conversation_id.as_str(), origin_type, origin_id, invocation],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .optional()
        .map_err(|error| storage(format!("lookup workflow run snapshot: {error}")))?;
    let Some((workflow_id, program_digest, source_revision, snapshot_digest, seq_raw)) = binding
    else {
        return Ok(None);
    };
    let committed_sequence = sequence_from_i64(seq_raw)?;
    if committed_sequence > cut.read.journal {
        // Bound after the caller's authorized immutable cut.
        return Ok(None);
    }

    verify_event_binding(connection, committed_sequence, &workflow_id, run_id)?;
    let (def_wf_id, def_prog, def_rev, has_pres_raw, byte_len_raw, sha256) =
        load_definition_metadata(connection, &snapshot_digest)?;
    if def_wf_id != workflow_id
        || def_prog != program_digest
        || def_rev != source_revision
        || !(0..=1).contains(&has_pres_raw)
        || byte_len_raw < 0
        || sha256.len() != 64
        || !sha256.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err(storage("corrupt workflow definition snapshot metadata"));
    }
    let byte_length = u64::try_from(byte_len_raw)
        .map_err(|_| storage("negative workflow snapshot byte length"))?;
    let max_bytes = u64::try_from(MAX_WORKFLOW_DOCUMENT_BYTES)
        .map_err(|_| storage("snapshot max bound overflow"))?;
    if byte_length > max_bytes {
        return Err(storage("stored workflow snapshot exceeds 16 MiB bound"));
    }
    let parsed_wf_id = WorkflowId::parse(&workflow_id)
        .map_err(|_| storage("corrupt workflow_id in snapshot binding"))?;
    let has_source_presentation = has_pres_raw == 1;
    let recomputed_digest = compute_snapshot_digest(
        &parsed_wf_id,
        &program_digest,
        &source_revision,
        has_source_presentation,
        &sha256,
    );
    if recomputed_digest != snapshot_digest {
        return Err(storage("workflow snapshot digest mismatch"));
    }

    Ok(Some(StoredSnapshotMeta {
        header: WorkflowRunSnapshotHeader {
            run_id: run_id.clone(),
            workflow_id: parsed_wf_id,
            program_digest,
            source_revision,
            snapshot_digest,
            has_source_presentation,
            byte_length,
            sha256,
        },
    }))
}

fn verify_event_binding(
    connection: &Connection,
    committed_sequence: u64,
    workflow_id: &str,
    run_id: &WorkflowRunId,
) -> Result<(), ConversationStoreError> {
    let event_check: Option<(String, String, String)> = connection
        .query_row(
            "SELECT json_extract(event_json, '$.event.type'),
                    json_extract(event_json, '$.event.workflow_id'),
                    json_extract(event_json, '$.event.run_id')
             FROM events WHERE sequence=?1",
            params![seq_to_i64(committed_sequence)?],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(|error| storage(format!("verify workflow snapshot event: {error}")))?;
    let Some((event_type, event_wf_id, event_run_json)) = event_check else {
        return Err(storage("workflow snapshot references missing event"));
    };
    let event_run: WorkflowRunId = serde_json::from_str(&event_run_json)
        .map_err(|_| storage("corrupt event run_id for workflow snapshot"))?;
    if event_type != "workflow_started" || event_wf_id != workflow_id || event_run != *run_id {
        return Err(storage("corrupt workflow run snapshot event binding"));
    }
    Ok(())
}

fn load_definition_metadata(
    connection: &Connection,
    snapshot_digest: &str,
) -> Result<(String, String, String, i64, i64, String), ConversationStoreError> {
    connection
        .query_row(
            "SELECT workflow_id, program_digest, source_revision,
                    has_source_presentation, byte_length, snapshot_sha256
             FROM workflow_definition_snapshots WHERE snapshot_digest=?1",
            params![snapshot_digest],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            },
        )
        .optional()
        .map_err(|error| {
            storage(format!(
                "load workflow definition snapshot metadata: {error}"
            ))
        })?
        .ok_or_else(|| storage("missing referenced workflow definition snapshot"))
}

pub(super) fn read_header(
    connection: &Connection,
    conversation_id: &ConversationId,
    cut: &ObservationReadCut,
    run_id: &WorkflowRunId,
) -> Result<Option<WorkflowRunSnapshotHeader>, ConversationStoreError> {
    Ok(load_header(connection, conversation_id, cut, run_id)?.map(|meta| meta.header))
}

pub(super) fn read_chunk(
    connection: &Connection,
    conversation_id: &ConversationId,
    cut: &ObservationReadCut,
    run_id: &WorkflowRunId,
    offset: u64,
    limit: usize,
) -> Result<Option<WorkflowRunSnapshotChunk>, ConversationStoreError> {
    if limit == 0 || limit > WORKFLOW_SNAPSHOT_CHUNK_BYTES {
        return Err(ConversationStoreError::InvalidReference(format!(
            "workflow snapshot chunk limit must be 1..={WORKFLOW_SNAPSHOT_CHUNK_BYTES}"
        )));
    }
    let Some(meta) = load_header(connection, conversation_id, cut, run_id)? else {
        return Ok(None);
    };
    let total = meta.header.byte_length;
    if offset > total {
        return Err(ConversationStoreError::InvalidReference(
            "workflow snapshot offset beyond byte_length".into(),
        ));
    }
    if offset == total {
        return Ok(Some(WorkflowRunSnapshotChunk {
            offset,
            data: String::new(),
            next_offset: None,
        }));
    }
    let chunk_size = u64::try_from(WORKFLOW_SNAPSHOT_CHUNK_BYTES).expect("chunk fits u64");
    let remaining = total - offset;
    let take = u64::try_from(limit).expect("limit fits u64").min(remaining);
    let end = offset + take;
    let first_chunk = offset / chunk_size;
    let last_chunk = (end - 1) / chunk_size;
    let mut out = Vec::with_capacity(usize::try_from(take).expect("take <= 65536"));

    for chunk_idx in first_chunk..=last_chunk {
        let chunk_idx_i64 =
            i64::try_from(chunk_idx).map_err(|_| storage("chunk index overflow"))?;
        let chunk_bytes: Option<Vec<u8>> = connection
            .query_row(
                "SELECT bytes FROM workflow_snapshot_chunks
                 WHERE snapshot_digest=?1 AND chunk_index=?2",
                params![meta.header.snapshot_digest, chunk_idx_i64],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| storage(format!("read workflow snapshot chunk: {error}")))?;
        let chunk_bytes = chunk_bytes.ok_or_else(|| storage("missing workflow snapshot chunk"))?;
        let expected_start = chunk_idx * chunk_size;
        let expected_chunk_len = usize::try_from((total - expected_start).min(chunk_size))
            .expect("chunk length <= 65536");
        if chunk_bytes.len() != expected_chunk_len {
            return Err(storage("corrupt workflow snapshot chunk length"));
        }
        let slice_start = if chunk_idx == first_chunk {
            usize::try_from(offset % chunk_size).expect("offset % chunk_size <= 65536")
        } else {
            0
        };
        let slice_end = if chunk_idx == last_chunk {
            let rel = end - expected_start;
            usize::try_from(rel).expect("rel <= 65536")
        } else {
            chunk_bytes.len()
        };
        out.extend_from_slice(&chunk_bytes[slice_start..slice_end]);
    }

    let next_offset = (end < total).then_some(end);
    Ok(Some(WorkflowRunSnapshotChunk {
        offset,
        data: base64::engine::general_purpose::STANDARD.encode(&out),
        next_offset,
    }))
}

pub(super) fn read_document(
    connection: &Connection,
    conversation_id: &ConversationId,
    cut: &ObservationReadCut,
    run_id: &WorkflowRunId,
) -> Result<Option<(WorkflowRunSnapshotHeader, WorkflowFileDocument)>, ConversationStoreError> {
    let Some(meta) = load_header(connection, conversation_id, cut, run_id)? else {
        return Ok(None);
    };
    let expected_len = usize::try_from(meta.header.byte_length)
        .map_err(|_| storage("workflow snapshot byte length overflow"))?;
    let expected_chunks = expected_len.div_ceil(WORKFLOW_SNAPSHOT_CHUNK_BYTES);
    let mut stmt = connection
        .prepare_cached(
            "SELECT chunk_index, bytes FROM workflow_snapshot_chunks
             WHERE snapshot_digest=?1 ORDER BY chunk_index ASC",
        )
        .map_err(|error| storage(format!("prepare workflow snapshot document read: {error}")))?;
    let rows = stmt
        .query_map(params![meta.header.snapshot_digest], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?))
        })
        .map_err(|error| storage(format!("query workflow snapshot chunks: {error}")))?;

    let mut buffer = Vec::with_capacity(expected_len);
    let mut expected_index = 0_i64;
    for row in rows {
        let (chunk_index, bytes) =
            row.map_err(|error| storage(format!("read chunk row: {error}")))?;
        if chunk_index != expected_index || buffer.len().saturating_add(bytes.len()) > expected_len
        {
            return Err(storage("corrupt workflow snapshot chunk sequence"));
        }
        buffer.extend_from_slice(&bytes);
        expected_index += 1;
    }
    if usize::try_from(expected_index).ok() != Some(expected_chunks) || buffer.len() != expected_len
    {
        return Err(storage("incomplete workflow snapshot chunks"));
    }
    if sha256_hex(&buffer) != meta.header.sha256 {
        return Err(storage("workflow snapshot checksum mismatch"));
    }
    let document: WorkflowFileDocument = serde_json::from_slice(&buffer)
        .map_err(|error| storage(format!("decode workflow snapshot document: {error}")))?;
    document.validate().map_err(|error| {
        storage(format!(
            "invalid stored workflow snapshot document: {}",
            error.reason
        ))
    })?;
    Ok(Some((meta.header, document)))
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use tempfile::TempDir;

    use crate::durable::sqlite::SqliteConversationStore;
    use crate::durable::{ConversationStoreError, JournalStore, ObservationStore};
    use crate::events::types::{EVENT_SCHEMA_VERSION, RuntimeEvent, RuntimeEventEnvelope};
    use crate::runtime::identity::{AttemptId, ConversationId, EventId};
    use crate::runtime::workflow::{
        WORKFLOW_SNAPSHOT_CHUNK_BYTES, WorkflowId, WorkflowProgram, WorkflowRunDefinitionSnapshot,
        WorkflowRunId, WorkflowRunOrigin,
    };
    use crate::workflow_file::WorkflowFileDocument;

    fn sample_document(with_editor: bool) -> WorkflowFileDocument {
        let mut document: WorkflowFileDocument = serde_json::from_str(include_str!(
            "../../../tests/fixtures/workflow/visual-authoring.json"
        ))
        .expect("visual authoring fixture");
        if with_editor {
            document.editor = Some(
                serde_json::from_value(serde_json::json!({
                    "nodes": [
                        {
                            "address": { "block": [], "node": "start" },
                            "position": { "x": 120.0, "y": 48.0 },
                            "title": "Admitted start"
                        }
                    ]
                }))
                .expect("editor presentation"),
            );
        } else {
            document.editor = None;
        }
        document
    }

    fn started_envelope(
        conversation_id: &ConversationId,
        workflow_id: &WorkflowId,
        run_id: &WorkflowRunId,
        event_id: &str,
    ) -> RuntimeEventEnvelope {
        RuntimeEventEnvelope {
            schema_version: EVENT_SCHEMA_VERSION,
            event_id: EventId::new(event_id),
            sequence: 0,
            tool_occurrence: None,
            observation: None,
            conversation_id: conversation_id.clone(),
            attempt_id: None,
            turn_id: None,
            timestamp: Utc::now(),
            event: RuntimeEvent::WorkflowStarted {
                tool_call_id: None,
                workflow_id: workflow_id.clone(),
                run_id: run_id.clone(),
            },
        }
    }

    fn make_run_id(
        conversation_id: &ConversationId,
        attempt: &str,
        invocation: u64,
    ) -> WorkflowRunId {
        WorkflowRunId {
            conversation_id: conversation_id.clone(),
            origin: WorkflowRunOrigin::Tool {
                attempt_id: AttemptId::new(attempt),
            },
            invocation,
        }
    }

    fn table_count(store: &SqliteConversationStore, table: &str) -> i64 {
        let conn = store.lock().expect("lock store");
        conn.query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
            row.get(0)
        })
        .expect("count rows")
    }

    #[test]
    fn atomic_rollback_on_snapshot_failure_leaves_zero_rows() {
        let conv = ConversationId::generate();
        let store = SqliteConversationStore::in_memory(conv.clone()).expect("in-memory store");
        let wf_id = WorkflowId::parse("review").expect("workflow id");
        let program =
            WorkflowProgram::compile_document(wf_id.clone(), "rev-a", sample_document(true))
                .expect("compile program");
        let run_id = make_run_id(&conv, "attempt-rollback", 0);
        let env = started_envelope(&conv, &wf_id, &run_id, "evt-start-rollback");

        store.arm_fail_workflow_snapshot_times(1);
        let err = store
            .publish_workflow_start(env.clone(), program.run_snapshot())
            .expect_err("injected snapshot publication failure should abort transaction");
        assert!(err.to_string().contains("injected"));
        assert_eq!(table_count(&store, "events"), 0);
        assert_eq!(table_count(&store, "workflow_definition_snapshots"), 0);
        assert_eq!(table_count(&store, "workflow_snapshot_chunks"), 0);
        assert_eq!(table_count(&store, "workflow_run_snapshots"), 0);

        // Clean retry publishes atomically and supports identical idempotent event_id replay.
        let committed = store
            .publish_workflow_start(env.clone(), program.run_snapshot())
            .expect("clean retry succeeds");
        assert_eq!(committed.sequence, 1);
        let replayed = store
            .publish_workflow_start(env, program.run_snapshot())
            .expect("identical replay succeeds idempotently");
        assert_eq!(replayed.sequence, 1);
        assert_eq!(table_count(&store, "events"), 1);
        assert_eq!(table_count(&store, "workflow_run_snapshots"), 1);
    }

    #[test]
    fn distinct_revisions_with_identical_program_digest_and_dedup() {
        let conv = ConversationId::generate();
        let store = SqliteConversationStore::in_memory(conv.clone()).expect("in-memory store");
        let wf_id = WorkflowId::parse("review").expect("workflow id");
        let prog_plain =
            WorkflowProgram::compile_document(wf_id.clone(), "rev-plain", sample_document(false))
                .expect("plain program");
        let prog_layout =
            WorkflowProgram::compile_document(wf_id.clone(), "rev-layout", sample_document(true))
                .expect("layout program");
        assert_eq!(
            prog_plain.run_snapshot().program_digest,
            prog_layout.run_snapshot().program_digest
        );
        let (_, _, digest_plain) =
            super::encode_snapshot_document(prog_plain.run_snapshot()).expect("encode plain");
        let (_, _, digest_layout) =
            super::encode_snapshot_document(prog_layout.run_snapshot()).expect("encode layout");
        assert_ne!(digest_plain, digest_layout);
        assert!(!prog_plain.run_snapshot().has_source_presentation);
        assert!(prog_layout.run_snapshot().has_source_presentation);

        let r0 = make_run_id(&conv, "att", 0);
        let r1 = make_run_id(&conv, "att", 1);
        let r2 = make_run_id(&conv, "att", 2);
        store
            .publish_workflow_start(
                started_envelope(&conv, &wf_id, &r0, "evt-0"),
                prog_plain.run_snapshot(),
            )
            .expect("publish r0");
        store
            .publish_workflow_start(
                started_envelope(&conv, &wf_id, &r1, "evt-1"),
                prog_layout.run_snapshot(),
            )
            .expect("publish r1");
        store
            .publish_workflow_start(
                started_envelope(&conv, &wf_id, &r2, "evt-2"),
                prog_plain.run_snapshot(),
            )
            .expect("publish r2 deduplicated definition");

        assert_eq!(table_count(&store, "workflow_definition_snapshots"), 2);
        assert_eq!(table_count(&store, "workflow_run_snapshots"), 3);

        let cut = store.observation_read_cut().expect("read cut");
        let (h0, d0) = store
            .observation_workflow_run_snapshot_document(&cut, &r0)
            .expect("read r0")
            .expect("r0 present");
        let (h1, d1) = store
            .observation_workflow_run_snapshot_document(&cut, &r1)
            .expect("read r1")
            .expect("r1 present");
        let (h2, _) = store
            .observation_workflow_run_snapshot_document(&cut, &r2)
            .expect("read r2")
            .expect("r2 present");

        assert_eq!(h0.source_revision, "rev-plain");
        assert!(!h0.has_source_presentation);
        assert!(d0.editor.is_none());
        assert_eq!(h1.source_revision, "rev-layout");
        assert!(h1.has_source_presentation);
        assert_eq!(
            d1.editor
                .expect("r1 editor")
                .nodes
                .first()
                .and_then(|n| n.title.as_deref()),
            Some("Admitted start")
        );
        assert_eq!(h2.snapshot_digest, h0.snapshot_digest);
    }

    fn build_256_node_document() -> WorkflowFileDocument {
        let mut nodes = serde_json::Map::new();
        nodes.insert("start".into(), serde_json::json!({ "type": "start" }));
        let mut edges = Vec::new();
        let mut prev = "start".to_owned();
        let long_code = format!("def main(): return {{'ok': True}} # {}", "x".repeat(420));
        for i in 1..=254 {
            let id = format!("n_{i:03}");
            nodes.insert(
                id.clone(),
                serde_json::json!({
                    "type": "code",
                    "code_language": "python3",
                    "variables": {},
                    "code": long_code,
                    "outputs": { "ok": "boolean" }
                }),
            );
            edges.push(serde_json::json!({ "from": prev, "to": id }));
            prev = id;
        }
        nodes.insert(
            "done".into(),
            serde_json::json!({ "type": "return", "outputs": [] }),
        );
        edges.push(serde_json::json!({ "from": prev, "to": "done" }));
        serde_json::from_value(serde_json::json!({
            "description": "256-node bounded benchmark workflow",
            "block": {
                "input": { "type": "object", "properties": {}, "additionalProperties": false },
                "nodes": nodes,
                "edges": edges
            }
        }))
        .expect("256-node document")
    }

    #[test]
    fn multichunk_256_node_streaming_bounds_and_corruption_detection() {
        use base64::Engine as _;

        let conv = ConversationId::generate();
        let store = SqliteConversationStore::in_memory(conv.clone()).expect("in-memory store");
        let wf_id = WorkflowId::parse("large_graph").expect("workflow id");
        let doc = build_256_node_document();

        let program = WorkflowProgram::compile_document(wf_id.clone(), "rev-256", doc)
            .expect("compile 256-node program");
        let snapshot: &WorkflowRunDefinitionSnapshot = program.run_snapshot();
        let (encoded_bytes, _, _) =
            super::encode_snapshot_document(snapshot).expect("encode 256-node snapshot");
        let encoded_byte_length = u64::try_from(encoded_bytes.len()).expect("len fits u64");
        assert!(encoded_byte_length > u64::try_from(WORKFLOW_SNAPSHOT_CHUNK_BYTES).unwrap());

        let run_id = make_run_id(&conv, "att-256", 0);
        store
            .publish_workflow_start(
                started_envelope(&conv, &wf_id, &run_id, "evt-256"),
                snapshot,
            )
            .expect("publish 256-node snapshot");
        let cut = store.observation_read_cut().expect("cut");
        let header = store
            .observation_workflow_run_snapshot_header(&cut, &run_id)
            .expect("header")
            .expect("present");
        assert_eq!(header.byte_length, encoded_byte_length);

        let mut reassembled = Vec::new();
        let mut offset = 0_u64;
        let mut chunk_reads = 0_usize;
        loop {
            let chunk = store
                .observation_workflow_run_snapshot_chunk(
                    &cut,
                    &run_id,
                    offset,
                    WORKFLOW_SNAPSHOT_CHUNK_BYTES,
                )
                .expect("read chunk")
                .expect("chunk present");
            chunk_reads += 1;
            let raw = base64::engine::general_purpose::STANDARD
                .decode(&chunk.data)
                .expect("valid base64");
            assert!(raw.len() <= WORKFLOW_SNAPSHOT_CHUNK_BYTES);
            reassembled.extend_from_slice(&raw);
            match chunk.next_offset {
                Some(next) => offset = next,
                None => break,
            }
        }
        assert!(chunk_reads >= 2);
        assert_eq!(
            reassembled.len(),
            usize::try_from(header.byte_length).unwrap()
        );
        assert_eq!(
            crate::runtime::workflow::snapshot::sha256_hex(&reassembled),
            header.sha256
        );

        // Bounds checks on chunk limit and offset.
        assert!(matches!(
            store.observation_workflow_run_snapshot_chunk(&cut, &run_id, 0, 0),
            Err(ConversationStoreError::InvalidReference(_))
        ));
        assert!(matches!(
            store.observation_workflow_run_snapshot_chunk(
                &cut,
                &run_id,
                0,
                WORKFLOW_SNAPSHOT_CHUNK_BYTES + 1
            ),
            Err(ConversationStoreError::InvalidReference(_))
        ));
        assert!(matches!(
            store.observation_workflow_run_snapshot_chunk(
                &cut,
                &run_id,
                header.byte_length + 1,
                64
            ),
            Err(ConversationStoreError::InvalidReference(_))
        ));

        // Corrupt a stored chunk and confirm fail-closed checksum rejection.
        {
            let db_conn = store.lock().expect("lock store");
            db_conn
                .execute(
                    "UPDATE workflow_snapshot_chunks SET bytes = zeroblob(length(bytes))
                 WHERE snapshot_digest=?1 AND chunk_index=0",
                    rusqlite::params![header.snapshot_digest],
                )
                .expect("corrupt chunk");
        }
        assert!(matches!(
            store.observation_workflow_run_snapshot_document(&cut, &run_id),
            Err(ConversationStoreError::Storage(_))
        ));
    }

    #[test]
    fn cut_authorization_cross_conversation_reopen_and_event_cascade() {
        let dir = TempDir::new().expect("tempdir");
        let db_path = dir.path().join("conv.db");
        let conv = ConversationId::generate();
        let other_conv = ConversationId::generate();
        let wf_id = WorkflowId::parse("review").expect("workflow id");
        let prog = WorkflowProgram::compile_document(wf_id.clone(), "rev-1", sample_document(true))
            .expect("compile");

        let run1 = make_run_id(&conv, "att-1", 0);
        let run2 = make_run_id(&conv, "att-2", 0);

        let cut_after_1 = {
            let store = SqliteConversationStore::open(conv.clone(), &db_path).expect("open store");
            store
                .publish_workflow_start(
                    started_envelope(&conv, &wf_id, &run1, "evt-1"),
                    prog.run_snapshot(),
                )
                .expect("publish run1");
            let cut_after_1 = store.observation_read_cut().expect("cut 1");

            store
                .publish_workflow_start(
                    started_envelope(&conv, &wf_id, &run2, "evt-2"),
                    prog.run_snapshot(),
                )
                .expect("publish run2");
            let cut_after_2 = store.observation_read_cut().expect("cut 2");

            // Cut after sequence 1 hides run2 (committed at sequence 2).
            assert!(
                store
                    .observation_workflow_run_snapshot_header(&cut_after_1, &run1)
                    .expect("header r1 at cut1")
                    .is_some()
            );
            assert!(
                store
                    .observation_workflow_run_snapshot_header(&cut_after_1, &run2)
                    .expect("header r2 at cut1")
                    .is_none()
            );
            assert!(
                store
                    .observation_workflow_run_snapshot_header(&cut_after_2, &run2)
                    .expect("header r2 at cut2")
                    .is_some()
            );

            // Cross-conversation run_id is explicitly denied.
            let foreign_run = make_run_id(&other_conv, "att-foreign", 0);
            assert!(matches!(
                store.observation_workflow_run_snapshot_header(&cut_after_2, &foreign_run),
                Err(ConversationStoreError::InvalidReference(_))
            ));
            cut_after_1
        };

        // Reopen existing read-only history store and verify durable recovery, then open writable for cascade check.
        {
            let reopened =
                SqliteConversationStore::open_existing(conv.clone(), &db_path).expect("reopen");
            let latest_cut = reopened.observation_read_cut().expect("reopened cut");
            assert!(
                reopened
                    .observation_workflow_run_snapshot_document(&latest_cut, &run2)
                    .expect("reopened r2 document")
                    .is_some()
            );
            assert!(
                reopened
                    .observation_workflow_run_snapshot_header(&cut_after_1, &run2)
                    .expect("cut 1 still hides r2 after reopen")
                    .is_none()
            );
        }

        let writable = SqliteConversationStore::open(conv, &db_path).expect("reopen writable");
        {
            let db_conn = writable.lock().expect("lock writable");
            db_conn
                .execute("DELETE FROM events WHERE sequence = 2", [])
                .expect("delete event 2");
        }
        let latest_cut = writable.observation_read_cut().expect("writable cut");
        assert!(
            writable
                .observation_workflow_run_snapshot_header(&latest_cut, &run2)
                .expect("cascade deleted run2 binding")
                .is_none()
        );
    }

    #[test]
    fn ordinary_append_event_rejects_workflow_started_without_leaving_rows() {
        let conv = ConversationId::generate();
        let store = SqliteConversationStore::in_memory(conv.clone()).expect("in-memory store");
        let wf_id = WorkflowId::parse("review").expect("workflow id");
        let run_id = make_run_id(&conv, "attempt-reject-append", 0);
        let env = started_envelope(&conv, &wf_id, &run_id, "evt-start-reject");
        let cut_before = store.observation_read_cut().expect("cut_before");

        let err = store
            .append_event(env)
            .expect_err("ordinary append_event must reject WorkflowStarted");
        assert!(matches!(err, ConversationStoreError::InvalidReference(_)));
        assert_eq!(table_count(&store, "events"), 0);
        assert_eq!(table_count(&store, "workflow_definition_snapshots"), 0);
        assert_eq!(table_count(&store, "workflow_snapshot_chunks"), 0);
        assert_eq!(table_count(&store, "workflow_run_snapshots"), 0);
        assert!(store.read_events(None, 16).unwrap().events.is_empty());
        let cut = store.observation_read_cut().expect("cut");
        assert_eq!(cut, cut_before);
        assert_eq!(cut.read.journal, 0);
        assert!(
            store
                .observation_workflow_trace_sequence(&cut, &run_id)
                .expect("trace sequence")
                .is_none()
        );
        assert!(
            store
                .observation_workflow_run_snapshot_header(&cut, &run_id)
                .expect("snapshot header")
                .is_none()
        );
    }
}
