use super::{Connection, ConversationStoreError, storage};
const TABLES: [(&str, &str); 3] = [
    (
        "request_source_pages",
        r"CREATE TABLE request_source_pages(hash BLOB PRIMARY KEY CHECK(typeof(hash)='blob' AND length(hash)=32),bytes BLOB NOT NULL CHECK(typeof(bytes)='blob' AND length(bytes)<=65536)) WITHOUT ROWID",
    ),
    (
        "request_source_nodes",
        r"CREATE TABLE request_source_nodes(native_id INTEGER PRIMARY KEY REFERENCES native_input_nodes(id),origin_source_id TEXT NOT NULL REFERENCES request_snapshots(observation_source_id),origin_rank INTEGER NOT NULL CHECK(origin_rank BETWEEN 0 AND 8191),native_digest BLOB NOT NULL CHECK(typeof(native_digest)='blob' AND length(native_digest)=32),hash BLOB NOT NULL CHECK(typeof(hash)='blob' AND length(hash)=32),metadata BLOB NOT NULL CHECK(typeof(metadata)='blob' AND length(metadata)<=65536))",
    ),
    (
        "request_source_roots",
        r"CREATE TABLE request_source_roots(source_id TEXT PRIMARY KEY REFERENCES request_snapshots(observation_source_id),metadata BLOB NOT NULL CHECK(typeof(metadata)='blob' AND length(metadata)<=16384),witness BLOB NOT NULL CHECK(typeof(witness)='blob' AND length(witness)=32)) WITHOUT ROWID",
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
    verify_root_seat(connection)
}

pub(super) fn verify_root_seat(connection: &Connection) -> Result<(), ConversationStoreError> {
    const EXPECTED: &str = "CREATE TABLE request_observation_fields (source_id TEXT PRIMARY KEY REFERENCES request_snapshots(observation_source_id),path_json TEXT NOT NULL CHECK(path_json='[]'),parent_path TEXT CHECK(parent_path IS NULL),ordinal INTEGER CHECK(ordinal IS NULL),start INTEGER NOT NULL CHECK(start=0),end INTEGER NOT NULL CHECK(end BETWEEN 0 AND 9007199254740991),children INTEGER NOT NULL CHECK(children=0),header_kind TEXT NOT NULL CHECK(header_kind='inline'),header_json BLOB NOT NULL CHECK(header_json=x'6e756c6c'),shape_witness BLOB NOT NULL CHECK(typeof(shape_witness)='blob' AND length(shape_witness)=32)) WITHOUT ROWID";
    let actual:Option<String>=connection.query_row("SELECT CASE WHEN typeof(sql)='text' AND length(CAST(sql AS BLOB))<=8192 THEN sql END FROM sqlite_schema WHERE type='table' AND name='request_observation_fields'",[],|row|row.get(0)).map_err(|error|storage(error.to_string()))?;
    let compact = |sql: &str| {
        sql.chars()
            .filter(|ch| !ch.is_ascii_whitespace())
            .collect::<String>()
    };
    if actual.as_deref().map(compact) != Some(compact(EXPECTED)) {
        return Err(ConversationStoreError::IncompatibleSchema(
            "original request witness schema differs".into(),
        ));
    }
    Ok(())
}
