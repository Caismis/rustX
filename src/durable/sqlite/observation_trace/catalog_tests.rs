use super::catalog::{BINDING_SQL, VERSION_SQL, index_catalog};
use crate::durable::sqlite::native_request::FreshConfiguration;
use crate::durable::sqlite::observation::tests::inspection::{
    inspection_assistant, inspection_request, inspection_result,
};
use crate::durable::sqlite::test_support::*;
use crate::durable::sqlite::*;
use crate::durable::{CanonicalStore, JournalStore, ObservationStore, RequestStore};
use chrono::Utc;
use rusqlite::StatementStatus;

fn unchanged_request(
    store: &SqliteConversationStore,
    first: &RequestSnapshot,
    retry: u32,
) -> (RequestSnapshot, u64) {
    let mut request = progress_request_snapshot(
        "inspection-original-owner",
        "1",
        retry,
        store.load_head().unwrap().revision,
        Vec::new(),
    );
    request.tool_definitions.clone_from(&first.tool_definitions);
    freeze_test_input(store, &mut request, &[]);
    let start = store
        .commit_model_turn_start(&[], &request, Utc::now())
        .unwrap()
        .started
        .sequence;
    (request, start)
}

fn catalog_id(connection: &Connection, start: u64) -> u64 {
    connection
        .query_row(
            "SELECT catalog_id FROM request_trace_catalogs WHERE source_sequence=?1",
            [start],
            |row| row.get(0),
        )
        .unwrap()
}

#[test]
fn unchanged_catalog_writes_one_binding_and_zero_definitions_at_growing_history_sizes() {
    let store = store();
    store.initialize(&[]).unwrap();
    let (first, start) = inspection_request(&store, 0, "frozen schema");
    let connection = store.lock().unwrap();
    connection.execute_batch(
        "CREATE TEMP TABLE catalog_writes(kind TEXT);
         CREATE TEMP TRIGGER catalog_binding_write AFTER INSERT ON main.request_trace_catalogs BEGIN INSERT INTO catalog_writes VALUES('binding'); END;
         CREATE TEMP TRIGGER catalog_version_write AFTER INSERT ON main.request_trace_catalog_versions BEGIN INSERT INTO catalog_writes VALUES('version'); END;
         CREATE TEMP TRIGGER catalog_definition_write AFTER INSERT ON main.request_trace_definitions BEGIN INSERT INTO catalog_writes VALUES('definition'); END;
         CREATE TEMP TRIGGER catalog_definition_rewrite BEFORE UPDATE ON main.request_trace_definitions BEGIN SELECT RAISE(ABORT,'immutable definitions rewritten'); END;
         CREATE TEMP TRIGGER catalog_definition_remove BEFORE DELETE ON main.request_trace_definitions BEGIN SELECT RAISE(ABORT,'immutable definitions removed'); END;",
    ).unwrap();
    let original_id = catalog_id(&connection, start);
    drop(connection);
    for retry in 1..=256 {
        let (_, next) = unchanged_request(&store, &first, retry);
        let connection = store.lock().unwrap();
        assert_eq!(catalog_id(&connection, next), original_id);
        if [1, 32, 128, 256].contains(&retry) {
            let writes: (u64, u64) = connection
                .query_row(
                    "SELECT count(*),sum(kind='binding') FROM catalog_writes",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .unwrap();
            assert_eq!(writes, (u64::from(retry), u64::from(retry)));
            let rows: (u64, u64) = connection.query_row(
                "SELECT (SELECT count(*) FROM request_trace_catalog_versions),(SELECT count(*) FROM request_trace_definitions)", [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            ).unwrap();
            assert_eq!(rows, (1, first.tool_definitions.len() as u64));
            // Grow the binding table itself, and exercise both its old and new
            // ends. Growing only the version table cannot catch a binding scan.
            for source in [start, next] {
                let mut lookup = connection.prepare(BINDING_SQL).unwrap();
                let ids: Vec<u64> = lookup
                    .query_map([source], |row| row.get(0))
                    .unwrap()
                    .collect::<Result<_, _>>()
                    .unwrap();
                assert_eq!(ids, [original_id]);
                assert!(lookup.get_status(StatementStatus::VmStep) < 100);
                assert_eq!(lookup.get_status(StatementStatus::FullscanStep), 0);
                assert_eq!(lookup.get_status(StatementStatus::Sort), 0);
            }
        }
    }
    let connection = store.lock().unwrap();
    connection
        .execute_batch(
            "DROP TRIGGER catalog_definition_rewrite; DROP TRIGGER catalog_definition_remove",
        )
        .unwrap();
    drop(connection);
    let (_, changed) = inspection_request(&store, 257, "different frozen schema");
    let connection = store.lock().unwrap();
    assert_ne!(catalog_id(&connection, changed), original_id);
    let writes: (u64, u64) = connection
        .query_row(
            "SELECT sum(kind='version'),sum(kind='definition') FROM catalog_writes",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(writes, (1, first.tool_definitions.len() as u64));
    drop(connection);
    assert_eq!(
        store.load_request_snapshot(&first.request_id).unwrap(),
        first
    );
}

#[test]
fn catalog_binding_and_version_seeks_have_constant_vm_work_as_history_grows() {
    let store = store();
    store.initialize(&[]).unwrap();
    let (first, start) = inspection_request(&store, 0, "frozen schema");
    let configuration = FreshConfiguration::capture(&first).unwrap();
    let connection = store.lock().unwrap();
    let mut previous = None;
    for size in [1, 32, 1024] {
        for index in 1..size {
            connection.execute(
                "INSERT OR IGNORE INTO request_trace_catalog_versions(configuration_digest,tool_count,complete) VALUES(?1,0,1)",
                [format!("{index:064x}")],
            ).unwrap();
        }
        let mut binding = connection.prepare(BINDING_SQL).unwrap();
        let ids: Vec<u64> = binding
            .query_map([start], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(ids, [catalog_id(&connection, start)]);
        let mut version = connection.prepare(VERSION_SQL).unwrap();
        let ids: Vec<u64> = version
            .query_map([configuration.identity()], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(ids, [catalog_id(&connection, start)]);
        let steps = (
            binding.get_status(StatementStatus::VmStep),
            version.get_status(StatementStatus::VmStep),
        );
        assert!(steps.0 < 100 && steps.1 < 50, "{size}: {steps:?}");
        for statement in [&binding, &version] {
            assert_eq!(statement.get_status(StatementStatus::FullscanStep), 0);
            assert_eq!(statement.get_status(StatementStatus::Sort), 0);
        }
        if let Some(previous) = previous {
            assert_eq!(steps, previous);
        }
        previous = Some(steps);
    }
    // Reusing a large unrelated version directory still writes only one row.
    let before = connection.total_changes();
    connection
        .execute_batch("SAVEPOINT repeat_binding")
        .unwrap();
    connection
        .execute(
            "DELETE FROM request_trace_catalogs WHERE source_sequence=?1",
            [start],
        )
        .unwrap();
    let deleted = connection.total_changes();
    connection
        .authorizer(Some(|context: rusqlite::hooks::AuthContext<'_>| {
            use rusqlite::hooks::{AuthAction, Authorization};
            if matches!(
                context.action,
                AuthAction::Read {
                    table_name: "request_trace_definitions" | "request_configurations",
                    ..
                }
            ) {
                Authorization::Deny
            } else {
                Authorization::Allow
            }
        }))
        .unwrap();
    index_catalog(&connection, start, &first, &configuration).unwrap();
    assert_eq!(connection.total_changes() - deleted, 1);
    assert_eq!(deleted - before, 1);
    connection
        .authorizer(None::<fn(rusqlite::hooks::AuthContext<'_>) -> rusqlite::hooks::Authorization>)
        .unwrap();
    connection
        .execute_batch("ROLLBACK TO repeat_binding; RELEASE repeat_binding")
        .unwrap();
}

#[test]
fn shared_catalog_keeps_original_first_ordinal_and_old_cut_after_reopen() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("catalog.sqlite");
    let conversation = ConversationId::generate();
    let store = SqliteConversationStore::open(conversation.clone(), &path).unwrap();
    store.initialize(&[]).unwrap();
    let (first, first_start) = inspection_request(&store, 0, "original schema");
    let (_, _, occurrence) = inspection_assistant(&store, &first);
    let cut = store.observation_read_cut().unwrap();
    let original = store
        .observation_trace_tool_definition(&cut, &occurrence)
        .unwrap()
        .unwrap();
    inspection_result(&store, &first);
    let (second, second_start) = unchanged_request(&store, &first, 1);
    inspection_assistant(&store, &second);
    let connection = store.lock().unwrap();
    assert_eq!(
        catalog_id(&connection, first_start),
        catalog_id(&connection, second_start)
    );
    connection
        .execute(
            "UPDATE request_configurations SET configuration_json='corrupt native configuration'",
            [],
        )
        .unwrap();
    drop(connection);
    assert!(store.load_request_snapshot(&first.request_id).is_err());
    drop(store);
    let store = SqliteConversationStore::open_existing(conversation, &path).unwrap();
    store.lock().unwrap().authorizer(Some(|context: rusqlite::hooks::AuthContext<'_>| {
        if matches!(context.action, rusqlite::hooks::AuthAction::Read { table_name: "request_configurations", .. }) {
            return rusqlite::hooks::Authorization::Deny;
        }
        crate::durable::sqlite::observation::tests::inspection::deny_inspection_original_columns(context)
    })).unwrap();
    assert_eq!(
        store
            .observation_trace_tool_definition(&cut, &occurrence)
            .unwrap()
            .unwrap(),
        original
    );
    assert!(matches!(
        original,
        crate::observation::trace::TraceFieldSource::Value { item_offset: 1, .. }
    ));
}

fn assert_damage_is_independent(damage: &str, before_capture: bool) {
    let store = store();
    store.initialize(&[]).unwrap();
    let (first, start) = inspection_request(&store, 0, "original schema");
    let (_, other) = inspection_request(&store, 1, "other schema");
    let captured = (!before_capture).then(|| inspection_assistant(&store, &first));
    let connection = store.lock().unwrap();
    let id = catalog_id(&connection, start);
    let mut statement = connection.prepare(damage).unwrap();
    let values = [id, start, catalog_id(&connection, other)];
    statement
        .execute(rusqlite::params_from_iter(
            &values[..statement.parameter_count()],
        ))
        .unwrap();
    drop(statement);
    drop(connection);
    let (_, event, occurrence) = captured.unwrap_or_else(|| inspection_assistant(&store, &first));
    let cut = store.observation_read_cut().unwrap();
    assert!(
        matches!(
            store.observation_trace_tool_definition(&cut, &occurrence),
            Err(ConversationStoreError::Storage(_))
        ),
        "{damage}, before={before_capture}"
    );
    assert!(
        store
            .observation_trace_tool_proposal(&cut, &occurrence)
            .is_ok()
    );
    assert!(
        store
            .request_field_header_at(start, cut.read.journal, &[])
            .is_ok()
    );
    assert_eq!(
        store.load_request_snapshot(&first.request_id).unwrap(),
        first
    );
    assert_eq!(
        store
            .read_events(Some(event.sequence - 1), 1)
            .unwrap()
            .events[0],
        event
    );
}

#[test]
fn missing_or_corrupt_shared_catalog_never_changes_native_or_other_trace_evidence() {
    for damage in [
        "DELETE FROM request_trace_catalog_versions WHERE id=?1",
        "DELETE FROM request_trace_definitions WHERE catalog_id=?1 AND ordinal=0",
        "INSERT INTO request_trace_definitions(catalog_id,ordinal,tool_identity_digest,tool_id) SELECT catalog_id,99,tool_identity_digest,tool_id FROM request_trace_definitions WHERE catalog_id=?1 LIMIT 1",
        "UPDATE request_trace_definitions SET tool_identity_digest=zeroblob(32) WHERE catalog_id=?1",
        "UPDATE request_trace_definitions SET tool_id='corrupt' WHERE catalog_id=?1",
        "UPDATE request_trace_catalogs SET catalog_id=?3 WHERE source_sequence=?2",
        "UPDATE request_trace_catalogs SET tool_count=tool_count+1 WHERE source_sequence=?2",
        "UPDATE request_trace_catalogs SET catalog_witness=zeroblob(32) WHERE source_sequence=?2",
        "UPDATE request_trace_catalog_versions SET configuration_digest=printf('%064d',0) WHERE id=?1",
        "UPDATE request_trace_catalog_versions SET tool_count=tool_count+1 WHERE id=?1",
    ] {
        for before_capture in [false, true] {
            assert_damage_is_independent(damage, before_capture);
        }
    }
}

#[test]
fn changed_version_identity_cannot_be_adopted_by_a_later_configuration() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("catalog-identity.sqlite");
    let conversation = ConversationId::generate();
    let store = SqliteConversationStore::open(conversation.clone(), &path).unwrap();
    store.initialize(&[]).unwrap();
    let (first, start) = inspection_request(&store, 0, "original schema");
    let (_, _, original_occurrence) = inspection_assistant(&store, &first);
    let original_cut = store.observation_read_cut().unwrap();
    inspection_result(&store, &first);
    let id = catalog_id(&store.lock().unwrap(), start);
    let mut changed = first.clone();
    // The true first matching ToolId is now ordinal zero. Reusing the old
    // members would silently return the unrelated definition at ordinal one.
    changed.tool_definitions.swap(0, 1);
    let configuration = FreshConfiguration::capture(&changed).unwrap();
    drop(store);
    let external = Connection::open(&path).unwrap();
    external
        .execute(
            "UPDATE request_trace_catalog_versions SET configuration_digest=?2 WHERE id=?1",
            params![id, configuration.identity()],
        )
        .unwrap();
    drop(external);
    let store = SqliteConversationStore::open(conversation, &path).unwrap();
    assert!(matches!(
        store.observation_trace_tool_definition(&original_cut, &original_occurrence),
        Err(ConversationStoreError::Storage(_))
    ));
    let (second, next) = unchanged_request(&store, &changed, 1);
    let (_, _, occurrence) = inspection_assistant(&store, &second);
    let cut = store.observation_read_cut().unwrap();
    let definition = store.observation_trace_tool_definition(&cut, &occurrence);
    assert!(
        matches!(definition, Err(ConversationStoreError::Storage(_))),
        "changed catalog identity was trusted: {definition:?}"
    );
    assert!(super::catalog::binding(&store.lock().unwrap(), next).is_err());
    assert_eq!(
        store.load_request_snapshot(&second.request_id).unwrap(),
        second
    );
    assert!(
        store
            .observation_trace_tool_proposal(&cut, &occurrence)
            .is_ok()
    );
    assert!(
        store
            .request_field_header_at(next, cut.read.journal, &[])
            .is_ok()
    );
}

#[test]
fn missing_version_is_never_backfilled_for_old_requests_by_a_later_request() {
    let store = store();
    store.initialize(&[]).unwrap();
    let (first, start) = inspection_request(&store, 0, "original schema");
    let (_, _, occurrence) = inspection_assistant(&store, &first);
    let cut = store.observation_read_cut().unwrap();
    inspection_result(&store, &first);
    let connection = store.lock().unwrap();
    let old = catalog_id(&connection, start);
    connection
        .execute(
            "DELETE FROM request_trace_catalog_versions WHERE id=?1",
            [old],
        )
        .unwrap();
    drop(connection);
    let (second, next) = unchanged_request(&store, &first, 1);
    let (_, _, next_occurrence) = inspection_assistant(&store, &second);
    assert_ne!(catalog_id(&store.lock().unwrap(), next), old);
    assert!(matches!(
        store.observation_trace_tool_definition(&cut, &occurrence),
        Err(ConversationStoreError::Storage(_))
    ));
    assert!(
        store
            .observation_trace_tool_definition(
                &store.observation_read_cut().unwrap(),
                &next_occurrence
            )
            .unwrap()
            .is_some()
    );
}

#[test]
fn failed_catalog_publication_rolls_back_and_a_later_request_cannot_rebind_it() {
    let store = store();
    store.initialize(&[]).unwrap();
    store.lock().unwrap().execute_batch("CREATE TRIGGER reject_catalog BEFORE INSERT ON request_trace_definitions WHEN NEW.ordinal=1 BEGIN SELECT RAISE(FAIL,'optional catalog unavailable'); END;").unwrap();
    let (first, start) = inspection_request(&store, 0, "original schema");
    let connection = store.lock().unwrap();
    let rows: (u64, u64, u64) = connection.query_row(
        "SELECT (SELECT count(*) FROM request_trace_catalogs),(SELECT count(*) FROM request_trace_catalog_versions),(SELECT count(*) FROM request_trace_definitions)", [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    ).unwrap();
    assert_eq!(rows, (0, 0, 0));
    connection
        .execute_batch("DROP TRIGGER reject_catalog")
        .unwrap();
    drop(connection);
    unchanged_request(&store, &first, 1);
    assert!(super::catalog::binding(&store.lock().unwrap(), start).is_err());
    assert_eq!(
        store.load_request_snapshot(&first.request_id).unwrap(),
        first
    );
}

#[test]
fn missing_member_cannot_claim_absence_or_be_repaired_by_catalog_reuse() {
    let store = store();
    store.initialize(&[]).unwrap();
    let (mut configuration, _) = inspection_request(&store, 0, "original schema");
    configuration.tool_definitions.truncate(1); // Only the unrelated ToolId remains.
    let (first, start) = unchanged_request(&store, &configuration, 1);
    let (_, _, occurrence) = inspection_assistant(&store, &first);
    let cut = store.observation_read_cut().unwrap();
    assert!(
        store
            .observation_trace_tool_definition(&cut, &occurrence)
            .unwrap()
            .is_none()
    );
    inspection_result(&store, &first);
    let connection = store.lock().unwrap();
    let id = catalog_id(&connection, start);
    connection
        .execute(
            "DELETE FROM request_trace_definitions WHERE catalog_id=?1",
            [id],
        )
        .unwrap();
    drop(connection);
    let (second, next) = unchanged_request(&store, &first, 2);
    assert!(super::catalog::binding(&store.lock().unwrap(), next).is_err());
    assert!(matches!(
        store.observation_trace_tool_definition(&cut, &occurrence),
        Err(ConversationStoreError::Storage(_))
    ));
    assert_eq!(
        store.load_request_snapshot(&second.request_id).unwrap(),
        second
    );
    assert!(
        !store
            .lock()
            .unwrap()
            .query_row(
                "SELECT complete FROM request_trace_catalog_versions WHERE id=?1",
                [id],
                |row| row.get::<_, bool>(0)
            )
            .unwrap()
    );
}

#[test]
fn catalog_schema_requires_every_durable_invalidation_trigger() {
    let store = store();
    let connection = store.lock().unwrap();
    for sql in super::catalog::INVALIDATIONS {
        connection
            .execute_batch("SAVEPOINT missing_catalog_trigger")
            .unwrap();
        let name = sql.split_whitespace().nth(2).unwrap();
        connection
            .execute_batch(&format!("DROP TRIGGER {name}"))
            .unwrap();
        assert!(matches!(
            super::verify_schema(&connection),
            Err(ConversationStoreError::IncompatibleSchema(_))
        ));
        connection
            .execute_batch("ROLLBACK TO missing_catalog_trigger; RELEASE missing_catalog_trigger")
            .unwrap();
        super::verify_schema(&connection).unwrap();
    }
}

#[test]
fn event_guard_accepts_exact_catalog_programs_and_rejects_a_same_named_custom_trigger() {
    let store = store();
    let connection = store.lock().unwrap();
    assert!(store.conn.event_guard.can_prepare(&connection).unwrap());
    for sql in super::catalog::INVALIDATIONS {
        connection
            .execute_batch("SAVEPOINT custom_catalog_trigger")
            .unwrap();
        let name = sql.split_whitespace().nth(2).unwrap();
        connection.execute_batch(&format!("DROP TRIGGER {name}; CREATE TRIGGER {name} AFTER INSERT ON events BEGIN SELECT NEW.sequence; END")).unwrap();
        assert!(!store.conn.event_guard.can_prepare(&connection).unwrap());
        connection
            .execute_batch("ROLLBACK TO custom_catalog_trigger; RELEASE custom_catalog_trigger")
            .unwrap();
        assert!(store.conn.event_guard.can_prepare(&connection).unwrap());
    }
}
