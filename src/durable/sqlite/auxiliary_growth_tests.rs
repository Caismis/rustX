use super::*;

use chrono::{TimeZone, Utc};
use rusqlite::Connection;

use crate::model::auxiliary::{
    AuxiliaryAuditCut, AuxiliaryBusinessSource, AuxiliaryCallOutcome, AuxiliaryCallStart, AuxiliaryAttribution, AuxiliaryCommitDisposition,
    AuxiliaryFingerprint, AuxiliaryFinishReason, AuxiliaryInputEvidence,
    AuxiliaryObservedProviderTerminal, AuxiliaryRequestEvidence, AuxiliaryRequestStart,
    AuxiliaryUsageCompleteness,
};
use crate::model::finish::ModelFinishReason;
use crate::model::snapshot::ModelRequestOwner;
use crate::model::types::ModelUsage;
use crate::runtime::identity::{AuxiliaryCallId, ConversationId, EventId, RequestId};

const MEASURED_CALLS: u64 = 100;
const FACTS_PER_CALL: u64 = 4;
const COMMIT_RECEIPTS_PER_CALL: u64 = 3;
const MEASUREMENT_MODEL: &str = "gpt-5-mini";

const AUXILIARY_TABLES: [&str; 4] = [
    "auxiliary_calls",
    "auxiliary_requests",
    "request_owners",
    "auxiliary_audit_facts",
];

const MANDATORY_AUXILIARY_INDEXES: [&str; 13] = [
    "auxiliary_requests_open_idx",
    "auxiliary_requests_request_idx",
    "auxiliary_requests_terminal_idx",
    "auxiliary_audit_offset_idx",
    "auxiliary_audit_call_idx",
    "auxiliary_audit_request_idx",
    "auxiliary_audit_sequence_idx",
    "auxiliary_calls_started_idx",
    "auxiliary_calls_settled_idx",
    "request_owners_source_idx",
    "events_auxiliary_call_id_kind_idx",
    "events_auxiliary_fact_type_idx",
    "events_primary_trace_kind_idx",
];

#[derive(Debug, Clone, Copy)]
struct PageMeasurements {
    page_size: u64,
    page_count: u64,
    freelist_count: u64,
    total_bytes: u64,
    live_bytes: u64,
}

#[derive(Debug, Clone, Copy)]
struct CheckpointMeasurements {
    busy: i64,
    log_frames: i64,
    checkpointed_frames: i64,
    pages: PageMeasurements,
}

fn measurement_timestamp() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 19, 8, 0, 0)
        .single()
        .unwrap()
}

fn measurement_id(prefix: &str, ordinal: u64) -> String {
    format!("{prefix}019a0000-0000-7000-8000-{ordinal:012x}")
}

fn measurement_fingerprint(ordinal: u64, salt: u64) -> AuxiliaryFingerprint {
    AuxiliaryFingerprint::new(format!("{:064x}", ordinal * 2 + salt)).unwrap()
}

fn measurement_usage() -> ModelUsage {
    ModelUsage {
        input_tokens: 8,
        output_tokens: 3,
        total_tokens: 11,
        details: None,
    }
}

fn auxiliary_envelope(
    store: &SqliteConversationStore,
    event_id: String,
    event: RuntimeEvent,
) -> RuntimeEventEnvelope {
    RuntimeEventEnvelope {
        tool_occurrence: None,
        observation: None,
        schema_version: EVENT_SCHEMA_VERSION,
        event_id: EventId::new(event_id),
        sequence: 0,
        conversation_id: store.conversation_id().clone(),
        attempt_id: None,
        turn_id: None,
        timestamp: measurement_timestamp(),
        event,
    }
}

fn commit_completed_one_request_call(
    store: &SqliteConversationStore,
    ordinal: u64,
) -> Result<(), String> {
    let call_id = AuxiliaryCallId::parse(measurement_id("aux_", ordinal))
        .map_err(|error| format!("fixture Call identity: {error}"))?;
    let request_id = RequestId::new(measurement_id("req_", ordinal));
    let timestamp = measurement_timestamp();

    let call = AuxiliaryCallStart {
        call_id: call_id.clone(),
        purpose: AuxiliaryAttribution::new("summary").unwrap(),
        trigger: AuxiliaryAttribution::new("automatic").unwrap(),
        business_source: AuxiliaryBusinessSource::Independent,
        input: AuxiliaryInputEvidence::Temporary,
        semantic_input_fingerprint: measurement_fingerprint(ordinal, 0),
        invocation_fingerprint: measurement_fingerprint(ordinal, 1),
        selected_message_ids: Vec::new(),
    };
    let call_receipt = store
        .commit_auxiliary_call_start(&call, timestamp)
        .map_err(|error| format!("commit Call start: {error:?}"))?;
    if call_receipt.disposition != AuxiliaryCommitDisposition::NewlyCommitted {
        return Err(format!(
            "Call {ordinal} was not newly committed: {:?}",
            call_receipt.disposition
        ));
    }

    let request = AuxiliaryRequestStart {
        call_id: call_id.clone(),
        ordinal: 0,
        request_id: request_id.clone(),
        model: MEASUREMENT_MODEL.into(),
    };
    let request_receipt = store
        .commit_auxiliary_request_start(&request, timestamp)
        .map_err(|error| format!("commit request start: {error:?}"))?;
    if request_receipt.disposition != AuxiliaryCommitDisposition::NewlyCommitted {
        return Err(format!(
            "request {ordinal} was not newly committed: {:?}",
            request_receipt.disposition
        ));
    }

    let usage = measurement_usage();
    let request_terminal = auxiliary_envelope(
        store,
        format!("auxiliary-request-completed:{request_id}"),
        RuntimeEvent::ModelRequestCompleted {
            owner: ModelRequestOwner::Auxiliary {
                call_id: call_id.clone(),
                ordinal: 0,
            },
            request_id: request_id.clone(),
            finish_reason: ModelFinishReason::Stop,
            usage: Some(usage.clone()),
            generation: None,
        },
    );
    let call_settled = auxiliary_envelope(
        store,
        format!("auxiliary-call-settled:{call_id}"),
        RuntimeEvent::AuxiliaryCallSettled {
            call_id,
            outcome: AuxiliaryCallOutcome::Completed,
            request_evidence: Some(AuxiliaryRequestEvidence {
                request_id,
                observed_provider_terminal: Some(AuxiliaryObservedProviderTerminal::Completed {
                    finish_reason: AuxiliaryFinishReason::Stop,
                }),
                usage: Some(usage),
                usage_completeness: AuxiliaryUsageCompleteness::Complete,
                remote_execution_uncertainty: false,
            }),
        },
    );
    let settlement = store
        .commit_auxiliary_request_settlement(request_terminal, call_settled)
        .map_err(|error| format!("commit request settlement: {error:?}"))?;
    if settlement.disposition != AuxiliaryCommitDisposition::NewlyCommitted {
        return Err(format!(
            "request {ordinal} settlement was not newly committed: {:?}",
            settlement.disposition
        ));
    }
    Ok(())
}

fn query_u64(connection: &Connection, sql: &str) -> u64 {
    let value: i64 = connection
        .query_row(sql, [], |row| row.get(0))
        .unwrap_or_else(|error| panic!("query {sql}: {error}"));
    u64::try_from(value).unwrap_or_else(|_| panic!("query {sql} returned negative {value}"))
}

fn checkpoint_and_measure_pages(store: &SqliteConversationStore) -> CheckpointMeasurements {
    let connection = store.lock().unwrap();
    let (busy, log_frames, checkpointed_frames): (i64, i64, i64) = connection
        .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .expect("checkpoint measured WAL");
    assert_eq!(busy, 0, "measurement checkpoint must not be busy");

    let page_size = query_u64(&connection, "PRAGMA page_size");
    let page_count = query_u64(&connection, "PRAGMA page_count");
    let freelist_count = query_u64(&connection, "PRAGMA freelist_count");
    assert!(
        page_count >= freelist_count,
        "SQLite freelist cannot exceed its page count"
    );
    CheckpointMeasurements {
        busy,
        log_frames,
        checkpointed_frames,
        pages: PageMeasurements {
            page_size,
            page_count,
            freelist_count,
            total_bytes: page_count * page_size,
            live_bytes: (page_count - freelist_count) * page_size,
        },
    }
}

fn added_journal_measurements(
    store: &SqliteConversationStore,
    after_sequence: u64,
) -> (u64, u64) {
    let connection = store.lock().unwrap();
    let after_sequence = i64::try_from(after_sequence).expect("Journal sequence fits i64");
    let journal_bytes = query_u64(
        &connection,
        &format!(
            "SELECT coalesce(sum(octet_length(event_json)),0) \
             FROM events WHERE sequence > {after_sequence}"
        ),
    );
    let added_events = query_u64(
        &connection,
        &format!("SELECT count(*) FROM events WHERE sequence > {after_sequence}"),
    );
    (journal_bytes, added_events)
}

fn fact_kind_counts(store: &SqliteConversationStore, after_sequence: u64) -> [u64; 4] {
    let connection = store.lock().unwrap();
    let after_sequence = i64::try_from(after_sequence).expect("Journal sequence fits i64");
    [
        query_u64(
            &connection,
            &format!(
                "SELECT count(*) FROM auxiliary_audit_facts \
                 WHERE sequence > {after_sequence} AND kind='call_started'"
            ),
        ),
        query_u64(
            &connection,
            &format!(
                "SELECT count(*) FROM auxiliary_audit_facts \
                 WHERE sequence > {after_sequence} AND kind='request_started'"
            ),
        ),
        query_u64(
            &connection,
            &format!(
                "SELECT count(*) FROM auxiliary_audit_facts \
                 WHERE sequence > {after_sequence} AND kind='request_settled'"
            ),
        ),
        query_u64(
            &connection,
            &format!(
                "SELECT count(*) FROM auxiliary_audit_facts \
                 WHERE sequence > {after_sequence} AND kind='call_settled'"
            ),
        ),
    ]
}

fn table_rows(store: &SqliteConversationStore) -> [u64; 4] {
    let connection = store.lock().unwrap();
    AUXILIARY_TABLES.map(|table| {
        query_u64(
            &connection,
            &format!("SELECT count(*) FROM {table}"),
        )
    })
}

fn mandatory_index_rows(store: &SqliteConversationStore) -> [u64; 13] {
    let connection = store.lock().unwrap();
    MANDATORY_AUXILIARY_INDEXES.map(|index| {
        let (table, sql): (String, String) = connection.query_row(
            "SELECT tbl_name,sql FROM sqlite_schema WHERE type='index' AND name=?1",
            [index], |row| Ok((row.get(0)?, row.get(1)?)),
        ).unwrap();
        let predicate = sql.find("WHERE").map_or("", |offset| &sql[offset..]);
        query_u64(
            &connection,
            &format!("SELECT count(*) FROM {table} INDEXED BY {index} {predicate}"),
        )
    })
}

fn expected_index_rows(calls: u64, facts: u64) -> [u64; 13] {
    [
        0,          // Every measured request is closed.
        calls,      // auxiliary_requests_request_idx
        calls,      // auxiliary_requests_terminal_idx
        facts,      // auxiliary_audit_offset_idx
        facts,      // auxiliary_audit_call_idx
        calls * 2,  // auxiliary_audit_request_idx: start and settlement
        facts,      // auxiliary_audit_sequence_idx
        calls,      // auxiliary_calls_started_idx
        calls,      // auxiliary_calls_settled_idx
        calls,      // request_owners_source_idx
        facts,      // Non-partial Call lookup also indexes null request Call keys.
        facts,      // events_auxiliary_fact_type_idx
        0,          // Primary Trace scope excludes every auxiliary request.
    ]
}

#[test]
#[allow(clippy::too_many_lines)] // Emit the complete reproducible storage measurement alongside its structural assertions.
fn aux_01_representative_storage_growth_of_completed_one_request_calls() {
    // The workload is deterministic; physical page sizes vary with the SQLite
    // build, so only structural counts and positive growth are asserted.
    let directory = tempfile::tempdir().unwrap();
    let store = SqliteConversationStore::open(
        ConversationId::new("conv_019a0000-0000-7000-8000-000000000000"),
        &directory.path().join("auxiliary-growth.sqlite"),
    )
    .unwrap();
    store.initialize(&[]).unwrap();

    let baseline_checkpoint = checkpoint_and_measure_pages(&store);
    let baseline_sequence = store.event_high_watermark().unwrap();
    let baseline_tables = table_rows(&store);
    let baseline_indexes = mandatory_index_rows(&store);
    assert_eq!(baseline_tables, [0, 0, 0, 0]);
    assert_eq!(baseline_indexes, [0; 13]);

    for ordinal in 0..MEASURED_CALLS {
        commit_completed_one_request_call(&store, ordinal).unwrap();
    }

    let final_checkpoint = checkpoint_and_measure_pages(&store);
    let final_sequence = store.event_high_watermark().unwrap();
    let (journal_bytes_added, added_events) =
        added_journal_measurements(&store, baseline_sequence);
    let fact_kinds = fact_kind_counts(&store, baseline_sequence);
    let tables = table_rows(&store);
    let indexes = mandatory_index_rows(&store);

    let expected_facts = MEASURED_CALLS * FACTS_PER_CALL;
    let expected_events = expected_facts;
    assert_eq!(final_sequence - baseline_sequence, expected_events);
    assert_eq!(added_events, expected_events);
    assert_eq!(
        tables,
        [MEASURED_CALLS, MEASURED_CALLS, MEASURED_CALLS, expected_facts]
    );
    assert_eq!(fact_kinds, [MEASURED_CALLS; 4]);
    assert_eq!(indexes, expected_index_rows(MEASURED_CALLS, expected_facts));
    assert_eq!(
        store
            .auxiliary_audit_total(AuxiliaryAuditCut {
                journal: final_sequence,
            })
            .unwrap(),
        expected_facts
    );

    let persisted_bytes_delta = final_checkpoint
        .pages
        .total_bytes
        .checked_sub(baseline_checkpoint.pages.total_bytes)
        .expect("persisted page bytes cannot shrink across measured Calls");
    let live_bytes_delta = final_checkpoint
        .pages
        .live_bytes
        .checked_sub(baseline_checkpoint.pages.live_bytes)
        .expect("live page bytes cannot shrink across measured Calls");
    let measurement = |value| f64::from(u32::try_from(value).expect("bounded fixture fits u32"));
    let persisted_bytes_per_call = measurement(persisted_bytes_delta) / measurement(MEASURED_CALLS);
    let persisted_bytes_per_fact = measurement(persisted_bytes_delta) / measurement(expected_facts);
    let journal_bytes_per_fact = measurement(journal_bytes_added) / measurement(expected_facts);
    let journal_bytes_per_call = measurement(journal_bytes_added) / measurement(MEASURED_CALLS);
    let index_rows_total: u64 = indexes.iter().sum();

    assert!(journal_bytes_added > 0);
    assert!(persisted_bytes_delta > 0);
    assert!(journal_bytes_per_fact.is_finite() && journal_bytes_per_fact > 0.0);
    assert!(persisted_bytes_per_call.is_finite() && persisted_bytes_per_call > 0.0);

    println!("AUX_GROWTH_CALLS={MEASURED_CALLS}");
    println!("AUX_GROWTH_COMPLETED_REQUESTS={MEASURED_CALLS}");
    println!("AUX_GROWTH_COMMIT_RECEIPTS={}", MEASURED_CALLS * COMMIT_RECEIPTS_PER_CALL);
    println!("AUX_GROWTH_COMMIT_RECEIPTS_PER_CALL={COMMIT_RECEIPTS_PER_CALL}");
    println!("AUX_GROWTH_JOURNAL_EVENTS_ADDED={added_events}");
    println!("AUX_GROWTH_AUDIT_FACTS={expected_facts}");
    println!(
        "AUX_GROWTH_FACT_ROWS={} call_started={} request_started={} request_settled={} call_settled={}",
        expected_facts, fact_kinds[0], fact_kinds[1], fact_kinds[2], fact_kinds[3]
    );
    println!(
        "AUX_GROWTH_TABLE_ROWS_auxiliary_calls={} AUX_GROWTH_TABLE_ROWS_auxiliary_requests={} \
         AUX_GROWTH_TABLE_ROWS_request_owners={} AUX_GROWTH_TABLE_ROWS_auxiliary_audit_facts={}",
        tables[0], tables[1], tables[2], tables[3]
    );
    for (index, rows) in MANDATORY_AUXILIARY_INDEXES.into_iter().zip(indexes) {
        println!("AUX_GROWTH_INDEX_ROWS_{index}={rows}");
    }
    println!("AUX_GROWTH_INDEX_ROWS_TOTAL={index_rows_total}");
    println!(
        "AUX_GROWTH_BASELINE_WAL_CHECKPOINT_BUSY={} LOG_FRAMES={} CHECKPOINTED_FRAMES={}",
        baseline_checkpoint.busy,
        baseline_checkpoint.log_frames,
        baseline_checkpoint.checkpointed_frames
    );
    println!(
        "AUX_GROWTH_FINAL_WAL_CHECKPOINT_BUSY={} LOG_FRAMES={} CHECKPOINTED_FRAMES={}",
        final_checkpoint.busy,
        final_checkpoint.log_frames,
        final_checkpoint.checkpointed_frames
    );
    println!(
        "AUX_GROWTH_BASELINE_PAGE_SIZE={} PAGE_COUNT={} FREELIST_COUNT={} TOTAL_BYTES={} LIVE_BYTES={}",
        baseline_checkpoint.pages.page_size,
        baseline_checkpoint.pages.page_count,
        baseline_checkpoint.pages.freelist_count,
        baseline_checkpoint.pages.total_bytes,
        baseline_checkpoint.pages.live_bytes
    );
    println!(
        "AUX_GROWTH_FINAL_PAGE_SIZE={} PAGE_COUNT={} FREELIST_COUNT={} TOTAL_BYTES={} LIVE_BYTES={}",
        final_checkpoint.pages.page_size,
        final_checkpoint.pages.page_count,
        final_checkpoint.pages.freelist_count,
        final_checkpoint.pages.total_bytes,
        final_checkpoint.pages.live_bytes
    );
    println!("AUX_GROWTH_LOGICAL_JOURNAL_BYTES_ADDED={journal_bytes_added}");
    println!("AUX_GROWTH_LOGICAL_JOURNAL_BYTES_PER_FACT={journal_bytes_per_fact:.2}");
    println!("AUX_GROWTH_LOGICAL_JOURNAL_BYTES_PER_CALL={journal_bytes_per_call:.2}");
    println!("AUX_GROWTH_PERSISTED_PAGE_BYTES_DELTA={persisted_bytes_delta}");
    println!("AUX_GROWTH_PERSISTED_LIVE_BYTES_DELTA={live_bytes_delta}");
    println!("AUX_GROWTH_PERSISTED_PAGE_BYTES_PER_CALL={persisted_bytes_per_call:.2}");
    println!("AUX_GROWTH_PERSISTED_PAGE_BYTES_PER_FACT={persisted_bytes_per_fact:.2}");
    println!(
        "AUX_GROWTH_PAGE_BYTES_MINUS_LOGICAL_JOURNAL_BYTES={}",
        persisted_bytes_delta.saturating_sub(journal_bytes_added)
    );
}
