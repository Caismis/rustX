use super::{Connection, ConversationStoreError, storage};
const TABLES: [(&str, &str); 4] = [
    (
        "request_source_pages",
        r"CREATE TABLE request_source_pages(hash BLOB NOT NULL PRIMARY KEY CHECK(typeof(hash)='blob' AND length(hash)=32),bytes BLOB NOT NULL CHECK(typeof(bytes)='blob' AND length(bytes)<=65536))",
    ),
    (
        "request_source_nodes",
        r"CREATE TABLE request_source_nodes(native_id INTEGER PRIMARY KEY REFERENCES native_input_nodes(id),native_digest BLOB NOT NULL CHECK(typeof(native_digest)='blob' AND length(native_digest)=32),hash BLOB NOT NULL CHECK(typeof(hash)='blob' AND length(hash)=32),metadata BLOB NOT NULL CHECK(typeof(metadata)='blob' AND length(metadata)<=65536))",
    ),
    (
        "request_source_scalars",
        r"CREATE TABLE request_source_scalars(identity BLOB NOT NULL PRIMARY KEY CHECK(typeof(identity)='blob' AND length(identity)=32),metadata BLOB NOT NULL CHECK(typeof(metadata)='blob' AND length(metadata)<=16384),checksum BLOB NOT NULL CHECK(typeof(checksum)='blob' AND length(checksum)=32)) WITHOUT ROWID",
    ),
    (
        "request_source_roots",
        r"CREATE TABLE request_source_roots(source_id TEXT PRIMARY KEY REFERENCES request_snapshots(observation_source_id),metadata BLOB NOT NULL CHECK(typeof(metadata)='blob' AND length(metadata)<=16384),checksum BLOB NOT NULL CHECK(typeof(checksum)='blob' AND length(checksum)=32)) WITHOUT ROWID",
    ),
];
pub(in crate::durable::sqlite) fn create_schema(
    connection: &Connection,
) -> Result<(), ConversationStoreError> {
    for (_, sql) in TABLES {
        connection
            .execute_batch(sql)
            .map_err(|error| storage(error.to_string()))?;
    }
    Ok(())
}
pub(in crate::durable::sqlite) fn verify_schema(
    connection: &Connection,
) -> Result<(), ConversationStoreError> {
    for (table, expected) in TABLES {
        let sql:Option<String>=connection.query_row("SELECT CASE WHEN typeof(sql)='text' AND length(CAST(sql AS BLOB))<=8192 THEN sql END FROM sqlite_schema WHERE type='table' AND name=?1",[table],|row|row.get(0)).map_err(|error|storage(error.to_string()))?;
        if sql.as_deref() != Some(expected) {
            return Err(ConversationStoreError::IncompatibleSchema(
                "composed observation schema differs".into(),
            ));
        }
    }
    Ok(())
}
