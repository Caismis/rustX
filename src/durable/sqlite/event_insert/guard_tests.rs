use super::*;

#[test]
fn event_guard_keeps_installed_proof_after_successful_and_rolled_back_workspace_spills() {
    use crate::durable::{ConversationStore, sqlite::SqliteConversationStore};
    let store = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
    store.initialize(&[]).unwrap();
    let connection = store.lock().unwrap();
    let installed = schema_versions(&connection).unwrap();
    assert_eq!(store.conn.event_guard.installed, Some(installed));
    GUARD_SCANS.with(|count| count.set(0));
    for committed in [true, false, true] {
        let transaction = connection.unchecked_transaction().unwrap();
        super::super::observation_request::exercise_workspace_spill(&transaction).unwrap();
        assert!(store.conn.event_guard.can_prepare(&transaction));
        if committed {
            transaction.commit().unwrap();
        } else {
            transaction.rollback().unwrap();
        }
        assert_eq!(schema_versions(&connection), Some(installed));
        assert!(store.conn.event_guard.can_prepare(&connection));
    }
    assert_eq!(GUARD_SCANS.with(std::cell::Cell::get), 0);
    assert_eq!(
        connection
            .query_row(
                "SELECT count(*) FROM request_source_fact_spool",
                [],
                |row| row.get::<_, u64>(0)
            )
            .unwrap(),
        0
    );
}

#[test]
fn event_guard_reuses_only_the_installed_main_and_temp_schema() {
    let connection = Connection::open_in_memory().unwrap();
    connection
        .execute_batch("CREATE TABLE target(value INTEGER)")
        .unwrap();
    let guard = Guard::new(&connection);
    assert!(guard.installed.is_some());
    GUARD_SCANS.with(|count| count.set(0));
    for _ in 0..32 {
        assert!(guard.can_prepare(&connection));
    }
    assert_eq!(GUARD_SCANS.with(std::cell::Cell::get), 0);
    for schema in ["", "TEMP "] {
        connection.execute_batch(&format!("CREATE {schema}TRIGGER observes_insert AFTER INSERT ON target BEGIN SELECT NEW.value; END")).unwrap();
        assert!(!guard.can_prepare(&connection));
        connection
            .execute_batch("DROP TRIGGER observes_insert")
            .unwrap();
        assert!(guard.can_prepare(&connection));
    }
    assert!(GUARD_SCANS.with(std::cell::Cell::get) >= 4);
}

#[test]
fn event_guard_does_not_seat_reused_savepoint_schema_versions() {
    let connection = Connection::open_in_memory().unwrap();
    connection
        .execute_batch("CREATE TABLE target(value INTEGER)")
        .unwrap();
    let guard = Guard::new(&connection);
    connection
        .execute_batch("SAVEPOINT change_schema; CREATE TABLE harmless(value INTEGER)")
        .unwrap();
    assert!(guard.can_prepare(&connection));
    let abandoned = schema_versions(&connection).unwrap();
    connection.execute_batch("ROLLBACK TO change_schema; RELEASE change_schema; CREATE TRIGGER must_observe AFTER INSERT ON target BEGIN SELECT NEW.value; END").unwrap();
    assert_eq!(schema_versions(&connection).unwrap(), abandoned);
    assert!(!guard.can_prepare(&connection));
    connection.execute_batch("DROP TRIGGER must_observe; SAVEPOINT temporary; CREATE TEMP TRIGGER temp_observer AFTER INSERT ON main.target BEGIN SELECT NEW.value; END").unwrap();
    assert!(!guard.can_prepare(&connection));
    connection
        .execute_batch("ROLLBACK TO temporary; RELEASE temporary")
        .unwrap();
    assert!(guard.can_prepare(&connection));
}

#[test]
fn event_guard_observes_external_schema_changes_in_the_current_snapshot() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("schema.sqlite");
    let connection = Connection::open(&path).unwrap();
    connection
        .execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE target(value INTEGER)")
        .unwrap();
    let guard = Guard::new(&connection);
    let writer = Connection::open(&path).unwrap();
    let transaction = connection.unchecked_transaction().unwrap();
    assert!(guard.can_prepare(&transaction));
    writer
        .execute_batch(
            "CREATE TRIGGER external_observer AFTER INSERT ON target BEGIN SELECT NEW.value; END",
        )
        .unwrap();
    // The existing snapshot has the old schema; the next one must observe DDL.
    assert!(guard.can_prepare(&transaction));
    transaction.commit().unwrap();
    assert!(!guard.can_prepare(&connection));
    writer
        .execute_batch("DROP TRIGGER external_observer")
        .unwrap();
    assert!(guard.can_prepare(&connection));
}

#[test]
fn event_guard_does_not_trust_an_initial_custom_trigger_or_a_cleanup_name() {
    let connection = Connection::open_in_memory().unwrap();
    connection.execute_batch("CREATE TABLE target(value INTEGER); CREATE TEMP TRIGGER event_readiness_source_deleted AFTER INSERT ON main.target BEGIN SELECT NEW.value; END").unwrap();
    let guard = Guard::new(&connection);
    assert!(guard.installed.is_none());
    assert!(!guard.can_prepare(&connection));
    connection
        .execute_batch("DROP TRIGGER event_readiness_source_deleted")
        .unwrap();
    assert!(guard.can_prepare(&connection));
}
