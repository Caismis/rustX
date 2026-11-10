use super::*;
use crate::durable::sqlite::schema::events::constraint;

fn write_bytes() -> u64 {
    std::fs::read_to_string("/proc/self/io")
        .unwrap()
        .lines()
        .find_map(|line| line.strip_prefix("write_bytes:"))
        .unwrap()
        .trim()
        .parse()
        .unwrap()
}

fn fixture(mode: &str) -> (tempfile::TempDir, SqliteConversationStore) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("identity.sqlite");
    let store = SqliteConversationStore::open(
        ConversationId::new("conv_36524fd8-f674-7fc2-8125-06d01fee0e18"),
        &path,
    )
    .unwrap();
    {
        let connection = store.lock().unwrap();
        if mode == "unchecked_partial" {
            connection
                .execute_batch("PRAGMA writable_schema=ON")
                .unwrap();
            connection
                .execute(
                    "UPDATE sqlite_schema SET sql=replace(sql,?1,?2) WHERE name='events'",
                    params![
                        constraint!(),
                        format!(
                            "{:<width$}",
                            "CONSTRAINT events_identity CHECK(1)",
                            width = constraint!().len()
                        )
                    ],
                )
                .unwrap();
            connection
                .execute_batch("PRAGMA writable_schema=OFF")
                .unwrap();
        } else if mode == "full_unique" {
            connection.execute_batch(
                "DROP INDEX events_alias_idx; CREATE UNIQUE INDEX events_alias_idx ON events(event_id)",
            ).unwrap();
        }
        let version: i64 = connection
            .query_row("PRAGMA schema_version", [], |row| row.get(0))
            .unwrap();
        connection
            .pragma_update(None, "schema_version", version + 1)
            .unwrap();
        connection
            .execute_batch("PRAGMA wal_autocheckpoint=0; PRAGMA wal_checkpoint(TRUNCATE)")
            .unwrap();
    }
    (directory, store)
}

fn measure(mode: &str) -> Vec<serde_json::Value> {
    let (directory, store) = fixture(mode);
    let initial_write_bytes = write_bytes();
    let mut samples = Vec::new();
    for turns in [8, 64, 512] {
        while store.event_high_watermark().unwrap() < turns {
            super::identity_tests::append(&store, "").unwrap();
        }
        let connection = store.lock().unwrap();
        let (rows, event_pages, identity_pages, identity_cells, identity_payload): (
            u64,
            u64,
            u64,
            u64,
            u64,
        ) = connection
            .query_row(
                "SELECT (SELECT count(*) FROM events),
                    (SELECT count(*) FROM dbstat WHERE name='events'),
                    (SELECT count(*) FROM dbstat WHERE name='events_alias_idx'),
                    (SELECT coalesce(sum(ncell),0) FROM dbstat WHERE name='events_alias_idx'),
                    (SELECT coalesce(sum(payload),0) FROM dbstat WHERE name='events_alias_idx')",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .unwrap();
        let page_size: u64 = connection
            .query_row("PRAGMA page_size", [], |row| row.get(0))
            .unwrap();
        let sqlite_version: String = connection
            .query_row("SELECT sqlite_version()", [], |row| row.get(0))
            .unwrap();
        let (conversation_pages, conversation_cells): (u64, u64) = connection
            .query_row(
                "SELECT count(*),sum(ncell) FROM dbstat WHERE name='rustx_store_conversation_idx'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!((conversation_pages, conversation_cells), (1, 1));
        assert_eq!(rows, turns);
        assert_eq!(identity_cells, if mode == "full_unique" { rows } else { 0 });
        samples.push(serde_json::json!({
            "mode":mode,"turns":turns,"rows":rows,"event_pages":event_pages,
            "identity_pages":identity_pages,"identity_cells":identity_cells,
            "identity_payload":identity_payload,"page_size":page_size,"sqlite_version":sqlite_version,
            "conversation_pages":conversation_pages,"conversation_cells":conversation_cells,
            "wal_bytes":std::fs::metadata(directory.path().join("identity.sqlite-wal")).unwrap().len(),
            "io_write_bytes":write_bytes()-initial_write_bytes,
        }));
    }
    samples
}

#[test]
fn checked_identity_retains_sparse_tree_and_write_savings() {
    let before = measure("unchecked_partial");
    let checked = measure("checked_partial");
    let full = measure("full_unique");
    for ((before, checked), full) in before.iter().zip(&checked).zip(&full) {
        for field in [
            "rows",
            "event_pages",
            "identity_pages",
            "identity_cells",
            "identity_payload",
            "wal_bytes",
        ] {
            assert_eq!(
                before[field], checked[field],
                "{field}: {before} vs {checked}"
            );
        }
        assert!(checked["wal_bytes"].as_u64().unwrap() < full["wal_bytes"].as_u64().unwrap());
        println!("{before}\n{checked}\n{full}");
    }
}
