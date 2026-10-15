use super::{Connection, ConversationStoreError, storage};
pub(in crate::durable::sqlite) fn create_schema(
    connection: &Connection,
) -> Result<(), ConversationStoreError> {
    connection.execute_batch("CREATE TABLE request_field_shapes(
        shape_id INTEGER PRIMARY KEY,origin_source_id TEXT NOT NULL,origin_item INTEGER NOT NULL CHECK(origin_item>=0),max_path_bytes INTEGER NOT NULL CHECK(max_path_bytes BETWEEN 1 AND 3600),producer_hash BLOB NOT NULL CHECK(typeof(producer_hash)='blob' AND length(producer_hash)=32),
        static_digest BLOB NOT NULL CHECK(typeof(static_digest)='blob' AND length(static_digest)=32),
        shape_hash BLOB NOT NULL CHECK(typeof(shape_hash)='blob' AND length(shape_hash)=32),
        metadata BLOB NOT NULL CHECK(typeof(metadata)='blob' AND length(metadata)<=65536),
        UNIQUE(producer_hash,static_digest),FOREIGN KEY(origin_source_id,origin_item) REFERENCES request_item_layouts(source_id,item_ordinal));
    CREATE TABLE request_item_layouts(
        source_id TEXT NOT NULL REFERENCES request_snapshots(observation_source_id),item_ordinal INTEGER NOT NULL CHECK(item_ordinal>=0),
        layout_kind TEXT NOT NULL CHECK(layout_kind IN ('own','shape')),
        producer_hash BLOB CHECK(producer_hash IS NULL OR (typeof(producer_hash)='blob' AND length(producer_hash)=32)),
        header_digest BLOB NOT NULL CHECK(typeof(header_digest)='blob' AND length(header_digest)=32),
        shape_id INTEGER REFERENCES request_field_shapes(shape_id),shape_hash BLOB,
        hole_hash BLOB,
        root_start INTEGER NOT NULL CHECK(root_start>=0),root_end INTEGER NOT NULL CHECK(root_end>=root_start),root_children INTEGER NOT NULL CHECK(root_children>=0),
        hole_start INTEGER,hole_end INTEGER,max_path_bytes INTEGER,hole_metadata BLOB,
        CHECK((layout_kind='own' AND hole_hash IS NULL AND hole_metadata IS NULL AND shape_id IS NULL AND shape_hash IS NULL AND hole_start IS NULL AND hole_end IS NULL AND max_path_bytes IS NULL)
            OR(layout_kind='shape' AND typeof(hole_hash)='blob' AND length(hole_hash)=32 AND typeof(hole_metadata)='blob' AND length(hole_metadata)<=16384 AND producer_hash IS NOT NULL AND hole_start IS NOT NULL AND hole_end IS NOT NULL AND max_path_bytes IS NOT NULL AND max_path_bytes BETWEEN 1 AND 3600 AND shape_id IS NOT NULL AND typeof(shape_hash)='blob' AND length(shape_hash)=32 AND hole_start>=root_start AND hole_end>hole_start AND hole_end<=root_end)),
        PRIMARY KEY(source_id,item_ordinal)) WITHOUT ROWID;")
        .map_err(|error| storage(error.to_string()))
}
pub(in crate::durable::sqlite::observation_request) fn verify_schema(
    connection: &Connection,
) -> Result<(), ConversationStoreError> {
    for (table, required) in [
        (
            "request_field_shapes",
            "typeof(metadata)='blob' AND length(metadata)<=65536",
        ),
        ("request_item_layouts", "layout_kind IN ('own','shape')"),
        (
            "request_item_layouts",
            "typeof(hole_metadata)='blob' AND length(hole_metadata)<=16384",
        ),
        (
            "request_observation_fields",
            "typeof(shape_witness)='blob' AND length(shape_witness)=32",
        ),
    ] {
        let sql: Option<String> = connection.query_row("SELECT CASE WHEN typeof(sql)='text' AND length(CAST(sql AS BLOB))<=65536 THEN sql END FROM sqlite_schema WHERE type='table' AND name=?1",[table],|row|row.get(0)).map_err(|error|storage(error.to_string()))?;
        let sql = sql.ok_or_else(|| storage("request shape schema unavailable"))?;
        if !sql.contains(required) {
            return Err(ConversationStoreError::IncompatibleSchema(
                "missing request shape layout".into(),
            ));
        }
    }
    Ok(())
}
