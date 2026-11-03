use super::*;

#[test]
fn compact_lookup_schema_refuses_a_shadow_rowid_tree() {
    let store = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
    let connection = store.lock().unwrap();
    for table in ["lifecycle_state", "observation_models"] {
        let definition: String = connection
            .query_row(
                "SELECT sql FROM sqlite_schema WHERE type='table' AND name=?1",
                [table],
                |row| row.get(0),
            )
            .unwrap();
        let rowid_definition = definition.strip_suffix("WITHOUT ROWID").unwrap();
        connection
            .execute_batch("SAVEPOINT wrong_lookup_tree")
            .unwrap();
        connection
            .execute_batch(&format!("DROP TABLE {table}; {rowid_definition}"))
            .unwrap();
        assert!(matches!(
            verify_schema_shape(&connection),
            Err(ConversationStoreError::IncompatibleSchema(reason))
                if reason == format!("compact primary tree required for {table}")
        ));
        connection
            .execute_batch("ROLLBACK TO wrong_lookup_tree; RELEASE wrong_lookup_tree")
            .unwrap();
        verify_schema_shape(&connection).unwrap();
    }
}
