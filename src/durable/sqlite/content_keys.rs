//! Compact, domain-local content coordinates. Public aliases are retained once;
//! canonical Ledger and Journal coordinates need no alias row or string index.
use super::{Connection, ConversationStoreError, OptionalExtension, storage};
use crate::durable::sqlite::statements::FixedStatements;

const ABSENT: i64 = i64::MIN;

fn coordinate(kind: &str, id: &str) -> Option<i64> {
    // Publication IDs are arbitrary public aliases, including numeric-looking
    // strings. Their spelling must never alias a native numeric coordinate.
    if matches!(
        kind,
        "live" | "live_block_metadata" | "audit" | "audit_block_metadata"
    ) {
        return None;
    }
    let (tag, digits) = if let Some(value) = id.strip_prefix("ledger:") {
        (1, value)
    } else if let Some(value) = id.strip_prefix("journal:") {
        (2, value)
    } else if let Some(value) = id.strip_prefix("request-start:") {
        (3, value)
    } else {
        (0, id)
    };
    if digits.is_empty()
        || (digits.len() > 1 && digits.starts_with('0'))
        || !digits.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    digits.parse::<i64>().ok()?.checked_mul(4)?.checked_add(tag)
}

pub(in crate::durable::sqlite) fn read(
    connection: &Connection,
    kind: &str,
    alias: &str,
) -> Result<i64, ConversationStoreError> {
    if let Some(id) = coordinate(kind, alias) {
        return Ok(id);
    }
    connection
        .query_row_cached(
            "SELECT -id FROM content_source_ids WHERE alias=?1",
            [alias],
            |row| row.get(0),
        )
        .optional()
        .map(|id| id.unwrap_or(ABSENT))
        .map_err(|error| storage(error.to_string()))
}

pub(in crate::durable::sqlite) fn write(
    connection: &Connection,
    kind: &str,
    alias: &str,
) -> Result<i64, ConversationStoreError> {
    let existing = read(connection, kind, alias)?;
    if existing != ABSENT {
        return Ok(existing);
    }
    // Negated, non-reused row IDs form a disjoint alias namespace. A removed
    // mapping can never give a later source ownership of an older byte carrier.
    connection
        .query_row_cached(
            "INSERT INTO content_source_ids(alias) VALUES(?1) RETURNING -id",
            [alias],
            |row| row.get(0),
        )
        .map_err(|error| storage(error.to_string()))
}

pub(in crate::durable::sqlite) fn verify_schema(
    connection: &Connection,
) -> Result<(), ConversationStoreError> {
    crate::durable::sqlite::schema::shape::verify_unique_columns(
        connection,
        "content_source_ids",
        &["alias"],
    )?;
    let sql: String = connection
        .query_row(
            "SELECT sql FROM sqlite_schema WHERE name='content_source_ids'",
            [],
            |row| row.get(0),
        )
        .map_err(|error| storage(error.to_string()))?;
    if !sql.contains("INTEGER PRIMARY KEY AUTOINCREMENT") {
        return Err(ConversationStoreError::IncompatibleSchema(
            "content aliases require non-reused numeric identities".into(),
        ));
    }
    for table in ["content_frontiers", "content_segments"] {
        let numeric:bool=connection.query_row("SELECT count(*)=1 FROM pragma_table_info(?1) WHERE name='source_id' AND type='INTEGER' AND [notnull]=1",[table],|row|row.get(0)).map_err(|error|storage(error.to_string()))?;
        if !numeric {
            return Err(ConversationStoreError::IncompatibleSchema(
                "numeric content source required".into(),
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::durable::sqlite::{query_work, schema};

    #[test]
    fn numeric_sources_are_exact_and_do_not_allocate_aliases() {
        let mut connection = Connection::open_in_memory().unwrap();
        schema::create_schema(&mut connection).unwrap();
        for (alias, expected) in [
            ("0", 0),
            ("42", 168),
            ("ledger:42", 169),
            ("journal:42", 170),
            ("request-start:42", 171),
        ] {
            assert_eq!(write(&connection, "canonical", alias).unwrap(), expected);
        }
        let count: u64 = connection
            .query_row("SELECT count(*) FROM content_source_ids", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 0);
        let stream = write(&connection, "live", "42").unwrap();
        assert!(stream < 0);
        assert_ne!(stream, read(&connection, "canonical", "42").unwrap());
        assert_ne!(write(&connection, "canonical", "042").unwrap(), 168);
        for alias in [
            "9223372036854775807",
            "18446744073709551615",
            "ledger:9223372036854775807",
        ] {
            let id = write(&connection, "canonical", alias).unwrap();
            assert!(id < 0);
            assert_eq!(read(&connection, "canonical", alias).unwrap(), id);
        }
    }

    #[test]
    fn committed_alias_ids_never_reuse_removed_carriers() {
        let mut connection = Connection::open_in_memory().unwrap();
        schema::create_schema(&mut connection).unwrap();
        let first = write(&connection, "live", "first").unwrap();
        connection
            .execute("DELETE FROM content_source_ids WHERE alias='first'", [])
            .unwrap();
        let next = write(&connection, "live", "next").unwrap();
        assert!(next < first);
        assert_eq!(read(&connection, "live", "first").unwrap(), ABSENT);
    }

    #[test]
    fn source_alias_queries_and_claims_have_constant_vm_work() {
        let mut hits = Vec::new();
        let mut misses = Vec::new();
        let mut writes = Vec::new();
        for history in [8, 64, 512, 2048] {
            let mut connection = Connection::open_in_memory().unwrap();
            schema::create_schema(&mut connection).unwrap();
            for i in 0..history {
                write(&connection, "live", &format!("stream-{i:06}")).unwrap();
            }
            hits.push((
                history,
                query_work::measure(
                    &connection,
                    "SELECT -id FROM content_source_ids WHERE alias=?1",
                    ["stream-000004"],
                ),
            ));
            misses.push((
                history,
                query_work::measure(
                    &connection,
                    "SELECT -id FROM content_source_ids WHERE alias=?1",
                    ["absent"],
                ),
            ));
            query_work::capture(&connection);
            write(&connection, "live", "next").unwrap();
            let work = query_work::finish(&connection)
                .into_iter()
                .map(|(sql, w)| (sql, (w.calls, w.steps, w.scans, w.sorts)))
                .collect::<std::collections::BTreeMap<_, _>>();
            writes.push(work);
        }
        query_work::assert_flat(&hits, 30, 1, 8);
        query_work::assert_flat(&misses, 30, 0, 0);
        for work in &writes[1..] {
            assert_eq!(work, &writes[0]);
        }
    }
}
