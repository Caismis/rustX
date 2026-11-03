use super::*;

#[test]
fn compact_lookup_schema_refuses_a_shadow_rowid_tree() {
    let store = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
    let connection = store.lock().unwrap();
    let table = "observation_models";
    {
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

#[test]
fn large_block_header_bodies_refuse_clustered_index_storage() {
    let store = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
    let connection = store.lock().unwrap();
    let definition: String = connection
        .query_row(
            "SELECT sql FROM sqlite_schema WHERE type='table' AND name='observation_block_headers'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    connection
        .execute_batch("SAVEPOINT wrong_header_tree")
        .unwrap();
    connection
        .execute_batch(&format!(
            "DROP TABLE observation_block_headers; {definition} WITHOUT ROWID"
        ))
        .unwrap();
    assert!(
        matches!(observation::verify_schema(&connection), Err(ConversationStoreError::IncompatibleSchema(reason)) if reason == "invalid finite native observation metadata layout")
    );
    connection
        .execute_batch("ROLLBACK TO wrong_header_tree; RELEASE wrong_header_tree")
        .unwrap();
    verify_schema_shape(&connection).unwrap();
}
