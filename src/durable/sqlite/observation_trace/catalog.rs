//! Immutable, independently available Trace catalogs. Native configuration is
//! only an admission-time identity; Trace reads never consult the native bytes.
use super::{Connection, ConversationStoreError, OptionalExtension, params, storage};
use crate::durable::sqlite::statements::FixedStatements;
use sha2::{Digest, Sha256};

// A sealed catalog is never repaired on reuse. Changes to any member invalidate
// the whole completeness proof, including negative ToolId lookups. The version
// identity must also remain sealed: a changed identity cannot let a future
// request adopt unrelated members. These durable triggers also observe edits
// made through another connection or after reopen.
pub(in crate::durable::sqlite) const INVALIDATIONS: [&str; 4] = [
    "CREATE TRIGGER request_trace_definition_inserted AFTER INSERT ON request_trace_definitions WHEN (SELECT complete FROM request_trace_catalog_versions WHERE id=NEW.catalog_id)=1 BEGIN UPDATE request_trace_catalog_versions SET complete=0 WHERE id=NEW.catalog_id; END",
    "CREATE TRIGGER request_trace_definition_updated AFTER UPDATE ON request_trace_definitions BEGIN UPDATE request_trace_catalog_versions SET complete=0 WHERE id IN (OLD.catalog_id,NEW.catalog_id); END",
    "CREATE TRIGGER request_trace_definition_deleted AFTER DELETE ON request_trace_definitions BEGIN UPDATE request_trace_catalog_versions SET complete=0 WHERE id=OLD.catalog_id; END",
    "CREATE TRIGGER request_trace_catalog_version_updated AFTER UPDATE OF id,configuration_digest,tool_count ON request_trace_catalog_versions BEGIN UPDATE request_trace_catalog_versions SET complete=0 WHERE id=NEW.id; END",
];

pub(in crate::durable::sqlite) fn create_schema(
    connection: &Connection,
) -> Result<(), ConversationStoreError> {
    for sql in INVALIDATIONS {
        connection
            .execute_batch(sql)
            .map_err(|error| crate::durable::sqlite::sqlite_error(&error, error.to_string()))?;
    }
    Ok(())
}

fn witness(start: u64, catalog: u64, count: u64, configuration: &str) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(b"rustx/trace/request-catalog-binding/v1\0");
    digest.update(start.to_be_bytes());
    digest.update(catalog.to_be_bytes());
    digest.update(count.to_be_bytes());
    digest.update(configuration.as_bytes());
    digest.finalize().into()
}

pub(super) struct Catalog {
    pub(super) id: u64,
    pub(super) count: u64,
}

type BindingRow = (
    u64,
    u64,
    Option<Vec<u8>>,
    Option<u64>,
    Option<String>,
    Option<bool>,
);
// Fixed scalar cells only: no tool IDs, native request JSON or configuration
// payloads enter a public Trace lookup, even for a missing definition.
pub(super) const BINDING_SQL: &str = "SELECT c.catalog_id,c.tool_count,CASE WHEN typeof(c.catalog_witness)='blob' AND length(c.catalog_witness)=32 THEN c.catalog_witness END,v.tool_count,CASE WHEN typeof(v.configuration_digest)='text' AND octet_length(v.configuration_digest)=64 THEN v.configuration_digest END,v.complete FROM request_trace_catalogs c LEFT JOIN request_trace_catalog_versions v ON v.id=c.catalog_id WHERE c.source_sequence=?1";
pub(super) const VERSION_SQL: &str = "SELECT id,tool_count,complete FROM request_trace_catalog_versions WHERE configuration_digest=?1";

pub(super) fn binding(
    connection: &Connection,
    start: u64,
) -> Result<Catalog, ConversationStoreError> {
    let row: Option<BindingRow> = connection
        .query_row_cached(BINDING_SQL, [start], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
            ))
        })
        .optional()
        .map_err(|error| crate::durable::sqlite::sqlite_error(&error, error.to_string()))?;
    let (id, count, stored, version_count, configuration, complete) =
        row.ok_or_else(|| storage("original Tool definition catalog binding unavailable"))?;
    let configuration = configuration
        .ok_or_else(|| storage("original Tool definition catalog version unavailable"))?;
    if complete != Some(true)
        || version_count != Some(count)
        || stored.as_deref() != Some(witness(start, id, count, &configuration).as_slice())
    {
        return Err(storage("original Tool definition catalog evidence differs"));
    }
    Ok(Catalog { id, count })
}

pub(in crate::durable::sqlite) fn index_catalog(
    connection: &Connection,
    start: u64,
    snapshot: &crate::model::RequestSnapshot,
    configuration: &crate::durable::sqlite::native_request::FreshConfiguration,
) -> Result<(), ConversationStoreError> {
    let count = snapshot.tool_definitions.len() as u64;
    let existing: Option<(u64, u64, bool)> = connection
        .query_row_cached(VERSION_SQL, [configuration.identity()], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .optional()
        .map_err(|error| crate::durable::sqlite::sqlite_error(&error, error.to_string()))?;
    let id = match existing {
        Some((id, retained, true)) if retained == count => id,
        Some(_) => {
            return Err(storage(
                "original Tool definition catalog evidence incomplete",
            ));
        }
        None => {
            connection.execute_cached(
                "INSERT INTO request_trace_catalog_versions(configuration_digest,tool_count,complete) VALUES(?1,?2,0)",
                params![configuration.identity(), count],
            ).map_err(|error| crate::durable::sqlite::sqlite_error(&error, error.to_string()))?;
            let id = connection.last_insert_rowid();
            let mut insert = connection.prepare_cached(
                "INSERT INTO request_trace_definitions(catalog_id,ordinal,tool_identity_digest,tool_id) VALUES(?1,?2,?3,?4)",
            ).map_err(|error| crate::durable::sqlite::sqlite_error(&error, error.to_string()))?;
            for (ordinal, definition) in snapshot.tool_definitions.iter().enumerate() {
                insert
                    .execute(params![
                        id,
                        ordinal as u64,
                        super::inspection::tool_identity_digest(&definition.id).as_slice(),
                        definition.id.as_str(),
                    ])
                    .map_err(|error| {
                        crate::durable::sqlite::sqlite_error(&error, error.to_string())
                    })?;
            }
            connection.execute_cached(
                "UPDATE request_trace_catalog_versions SET complete=1 WHERE id=?1 AND (SELECT count(*) FROM request_trace_definitions WHERE catalog_id=?1)=tool_count",
                [id],
            ).map_err(|error| crate::durable::sqlite::sqlite_error(&error, error.to_string()))?;
            u64::try_from(id).map_err(|error| storage(error.to_string()))?
        }
    };
    connection.execute_cached(
        "INSERT INTO request_trace_catalogs(source_sequence,catalog_id,tool_count,catalog_witness,provisional_message_id) VALUES(?1,?2,?3,?4,?5)",
        params![start,id,count,witness(start,id,count,configuration.identity()).as_slice(),snapshot.provisional_message_id.as_str()],
    ).map_err(|error| crate::durable::sqlite::sqlite_error(&error, error.to_string()))?;
    // Trigger effects during initial publication must not advertise a partial
    // version as available. All new rows roll back with the optional savepoint.
    binding(connection, start)?;
    Ok(())
}
