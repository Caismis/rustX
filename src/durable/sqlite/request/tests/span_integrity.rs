//! Request snapshot and model-turn start regressions: span integrity.

use crate::durable::ConversationStoreError;
use crate::durable::sqlite::test_support::*;
use crate::durable::sqlite::*;
use crate::durable::{CanonicalStore, JournalStore, ObservationStore, RequestStore};
use crate::model::snapshot::RequestSnapshot;
use chrono::Utc;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

pub(super) fn span_integrity_fixture() -> (SqliteConversationStore, RequestSnapshot, u64) {
    let store = store();
    store
        .initialize(&[user_message("span-native", "original canonical body")])
        .unwrap();
    let mut snapshot = test_request_snapshot(store.load_head().unwrap().revision, invocation());
    freeze_test_input(&store, &mut snapshot, &[]);
    let sequence = store
        .commit_model_turn_start(&[], &snapshot, Utc::now())
        .unwrap()
        .started
        .sequence;
    (store, snapshot, sequence)
}
pub(super) fn assert_native_span_authority(
    store: &SqliteConversationStore,
    snapshot: &RequestSnapshot,
    sequence: u64,
) {
    assert_eq!(
        store.load_request_snapshot(&snapshot.request_id).unwrap(),
        *snapshot
    );
    assert_eq!(store.presentation_frontier().unwrap(), sequence);
    assert_eq!(
        store.load_canonical().unwrap(),
        vec![user_message("span-native", "original canonical body")]
    );
    assert_eq!(
        store
            .request_value_read_at(sequence, sequence, &["retry_number".into()], 0, 64)
            .unwrap()
            .bytes,
        b"0"
    );
}

#[test]
fn t08_t19_composed_source_corruption_preserves_native_request_and_restored_exact_ranges() {
    for mutation in [
        "metadata=zeroblob(4000000)",
        "metadata='{}'",
        "metadata=x'7b7d'",
        "witness=zeroblob(32)",
    ] {
        let (store, snapshot, sequence) = span_integrity_fixture();
        let connection = store.lock().unwrap();
        connection
            .execute_batch("SAVEPOINT corrupt_source; PRAGMA ignore_check_constraints=ON")
            .unwrap();
        assert_eq!(
            connection
                .execute(
                    &format!("UPDATE request_source_roots SET {mutation} WHERE source_id=?1"),
                    [sequence.to_string()]
                )
                .unwrap(),
            1
        );
        assert!(
            matches!(
                observation_request::field_header_at(
                    &connection,
                    sequence,
                    sequence,
                    &["retry_number".into()]
                ),
                Err(ConversationStoreError::Storage(_))
            ),
            "{mutation}"
        );
        connection.execute_batch("ROLLBACK TO corrupt_source; RELEASE corrupt_source; PRAGMA ignore_check_constraints=OFF").unwrap();
        drop(connection);
        assert_native_span_authority(&store, &snapshot, sequence);
        assert_eq!(
            store
                .request_field_header_at(sequence, sequence, &["retry_number".into()])
                .unwrap()
                .value,
            serde_json::json!(0)
        );
    }
}

pub(super) fn assert_independent_request_start_fact(
    store: &SqliteConversationStore,
    sequence: u64,
    snapshot: &RequestSnapshot,
) {
    use crate::durable::observation::{
        TraceFactData, TraceFactKind, TraceFactQuery, TraceFactScope,
    };
    let cut = store.observation_read_cut().unwrap();
    let facts = store
        .observation_trace_facts(
            &cut,
            &TraceFactQuery {
                scope: TraceFactScope::All,
                kinds: vec![TraceFactKind::ModelRequestStarted],
                before: None,
                after: 0,
                ascending: true,
                limit: 1,
            },
        )
        .unwrap();
    assert_eq!(facts.len(), 1);
    assert_eq!(facts[0].sequence, sequence);
    assert!(matches!(&facts[0].fact,TraceFactData::RequestStart {
        start_sequence,request_id:Some(request),..
    } if *start_sequence==sequence && request==&snapshot.request_id));
    let flags:(u64,u64)=store.lock().unwrap().query_row(
        "SELECT observation_unavailable,observation_readiness_missing FROM events WHERE sequence=?1",
        [sequence],|row|Ok((row.get(0)?,row.get(1)?)),
    ).unwrap();
    assert_eq!(
        flags,
        (0, 0),
        "independent native J evidence remains available"
    );
    assert!(matches!(
        store.observation_request_summary_at(sequence, sequence),
        Err(ConversationStoreError::Storage(_))
    ));
}

#[test]
fn t10_t19_original_span_insert_failure_rolls_back_optional_layout_and_preserves_native_start() {
    let store = store();
    store
        .initialize(&[user_message("span-native", "original canonical body")])
        .unwrap();
    let mut snapshot = test_request_snapshot(store.load_head().unwrap().revision, invocation());
    freeze_test_input(&store, &mut snapshot, &[]);
    let attempts = Arc::new(AtomicUsize::new(0));
    let observed = attempts.clone();
    {
        let connection = store.lock().unwrap();
        connection
            .create_scalar_function(
                "test_original_span_attempt",
                0,
                rusqlite::functions::FunctionFlags::SQLITE_UTF8,
                move |_| {
                    observed.fetch_add(1, Ordering::SeqCst);
                    Ok(0_i64)
                },
            )
            .unwrap();
        connection.execute_batch("CREATE TRIGGER reject_original_span BEFORE INSERT ON request_observation_fields BEGIN SELECT test_original_span_attempt(); SELECT RAISE(FAIL,'optional span insert'); END;").unwrap();
    }
    let started = store
        .commit_model_turn_start(&[], &snapshot, Utc::now())
        .unwrap()
        .started;
    assert_eq!(
        attempts.load(Ordering::SeqCst),
        1,
        "the real optional insert trigger fired"
    );
    assert!(!started.observation.as_ref().unwrap().source_unavailable);
    assert_independent_request_start_fact(&store, started.sequence, &snapshot);
    assert_eq!(
        store.load_request_snapshot(&snapshot.request_id).unwrap(),
        snapshot
    );
    assert_eq!(
        store.load_canonical().unwrap(),
        vec![user_message("span-native", "original canonical body")]
    );
    assert_eq!(store.presentation_frontier().unwrap(), started.sequence);
    assert!(
        store
            .request_field_header_at(started.sequence, started.sequence, &[])
            .is_err()
    );
    let connection = store.lock().unwrap();
    for sql in [
        "SELECT count(*) FROM request_observation_fields WHERE source_id=?1",
        "SELECT count(*) FROM content_segments WHERE source_kind='trace_request' AND source_id=?1",
        "SELECT count(*) FROM content_frontiers WHERE source_kind='trace_request' AND source_id=?1",
    ] {
        assert_eq!(
            connection
                .query_row(sql, [started.sequence.to_string()], |row| row
                    .get::<_, u64>(0))
                .unwrap(),
            0
        );
    }
    drop(connection);
    let (healthy, same, sequence) = span_integrity_fixture();
    assert_native_span_authority(&healthy, &same, sequence);
    assert!(
        !healthy
            .request_field_header_at(sequence, sequence, &["retry_number".into()])
            .unwrap()
            .deferred
    );
}

#[test]
fn t14_t19_original_span_schema_requires_explicit_kind_and_consistent_native_bounds() {
    let (store, snapshot, sequence) = span_integrity_fixture();
    let connection = store.lock().unwrap();
    for mutation in [
        "header_kind='unknown'",
        "header_kind=NULL",
        "header_json=x'30'",
        "path_json='[1]'",
        "parent_path='[]'",
        "ordinal=0",
        "start=1",
        "end=9007199254740992",
        "children=1",
        "shape_witness=zeroblob(31)",
    ] {
        assert!(connection.execute(&format!("UPDATE request_observation_fields SET {mutation} WHERE source_id=?1 AND path_json='[]'"), [sequence.to_string()]).is_err(), "{mutation}");
    }
    assert!(connection.execute("INSERT INTO request_observation_fields(source_id,path_json,start,end,children,header_json) VALUES(?1,'[]',0,1,0,x'30')", [sequence.to_string()]).is_err());
    drop(connection);
    assert_native_span_authority(&store, &snapshot, sequence);
}

#[derive(schemars::JsonSchema)]
pub(super) struct DuplicateOrdinalKeys;
impl serde::Serialize for DuplicateOrdinalKeys {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap as _;
        let mut map = serializer.serialize_map(Some(2))?;
        map.serialize_entry("@same", &1)?;
        map.serialize_entry("@same", &2)?;
        map.end()
    }
}

#[test]
fn t08_t19_original_span_duplicate_ordinal_keys_preserve_legacy_last_wins_header() {
    let (store, snapshot, sequence) = span_integrity_fixture();
    let mut connection = store.lock().unwrap();
    let transaction = connection.transaction().unwrap();
    transaction
        .execute(
            "DELETE FROM request_observation_fields WHERE source_id=?1",
            [sequence.to_string()],
        )
        .unwrap();
    transaction
        .execute(
            "DELETE FROM content_segments WHERE source_kind='trace_request' AND source_id=?1",
            [sequence.to_string()],
        )
        .unwrap();
    transaction
        .execute(
            "DELETE FROM content_frontiers WHERE source_kind='trace_request' AND source_id=?1",
            [sequence.to_string()],
        )
        .unwrap();
    observation_request::index_test_request_fields(&transaction, sequence, &DuplicateOrdinalKeys)
        .unwrap();
    let root = observation_request::field_from(
        &transaction,
        "request_observation_fields",
        &sequence.to_string(),
        &[],
    )
    .unwrap();
    assert_eq!(root.3, br#"{"@same":2}"#);
    let raw = observation_request::read_from(
        &transaction,
        "trace_request",
        "request_observation_fields",
        &sequence.to_string(),
        &[],
        0,
        64,
    )
    .unwrap();
    assert_eq!(raw.bytes, br#"{"@same":1,"@same":2}"#);
    let page = observation_request::page_from(
        &transaction,
        "request_observation_fields",
        &sequence.to_string(),
        &[],
        0,
        32,
    )
    .unwrap();
    assert_eq!(page.total, 2);
    assert_eq!(page.fields[0].path, vec!["@0"]);
    assert_eq!(page.fields[1].path, vec!["@1"]);
    assert_eq!(page.fields[0].value, serde_json::json!(1));
    assert_eq!(page.fields[1].value, serde_json::json!(2));
    assert!(page.next_offset.is_none());
    transaction.rollback().unwrap();
    drop(connection);
    assert_native_span_authority(&store, &snapshot, sequence);
}
