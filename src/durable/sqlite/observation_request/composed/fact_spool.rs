//! Small producers sort bounded owned facts; large producers spill to SQLite.
use super::{Connection, ConversationStoreError, index, params, storage};
const MAX_BYTES: usize = 65_536;
const MAX_FACTS: usize = 256;
pub(super) struct FactSpool<'a> {
    connection: &'a Connection,
    facts: Vec<(String, Vec<u8>)>,
    bytes: usize,
    spilled: bool,
}
impl<'a> FactSpool<'a> {
    pub(super) fn new(connection: &'a Connection) -> Self {
        Self {
            connection,
            facts: Vec::new(),
            bytes: 0,
            spilled: false,
        }
    }
    fn disk_insert(&self, key: &str, value: &[u8]) -> Result<(), ConversationStoreError> {
        self.connection
            .prepare_cached("INSERT INTO request_source_fact_spool(key,value) VALUES(?1,?2)")
            .map_err(|error| storage(error.to_string()))?
            .execute(params![key, value])
            .map_err(|error| storage(error.to_string()))?;
        Ok(())
    }
    fn spill(&mut self) -> Result<(), ConversationStoreError> {
        self.connection.execute_batch("CREATE TEMP TABLE IF NOT EXISTS request_source_fact_spool(key TEXT PRIMARY KEY,value BLOB NOT NULL) WITHOUT ROWID; DELETE FROM request_source_fact_spool;")
            .map_err(|error|storage(error.to_string()))?;
        for (key, value) in std::mem::take(&mut self.facts) {
            self.disk_insert(&key, &value)?;
        }
        self.bytes = 0;
        self.spilled = true;
        Ok(())
    }
    pub(super) fn push(
        &mut self,
        key: String,
        value: Vec<u8>,
    ) -> Result<(), ConversationStoreError> {
        if key.len() > 4160 || value.len() > 40960 {
            return Err(storage("fragment fact exceeds bound"));
        }
        if !self.spilled {
            if self.facts.iter().any(|(existing, _)| existing == &key) {
                return Err(storage("fragment fact repeats"));
            }
            // Capacity accounts for owned buffers as well as encoded lengths.
            let size = key.capacity().saturating_add(value.capacity());
            if self.facts.len() < MAX_FACTS
                && self.bytes.checked_add(size).is_some_and(|n| n <= MAX_BYTES)
            {
                self.bytes += size;
                self.facts.push((key, value));
                return Ok(());
            }
            self.spill()?;
        }
        self.disk_insert(&key, &value)
    }
    pub(super) fn finish(mut self) -> Result<index::Reference, ConversationStoreError> {
        let mut builder = index::Builder::new(self.connection);
        if self.spilled {
            let mut statement = self
                .connection
                .prepare_cached("SELECT key,value FROM request_source_fact_spool ORDER BY key")
                .map_err(|error| storage(error.to_string()))?;
            let mut rows = statement
                .query([])
                .map_err(|error| storage(error.to_string()))?;
            while let Some(row) = rows.next().map_err(|error| storage(error.to_string()))? {
                builder.push(
                    row.get(0).map_err(|error| storage(error.to_string()))?,
                    row.get(1).map_err(|error| storage(error.to_string()))?,
                )?;
            }
        } else {
            // Rust UTF-8 byte ordering is SQLite BINARY ordering. Unstable sort
            // allocates no scratch buffer; duplicate keys were rejected on push.
            self.facts.sort_unstable_by(|a, b| a.0.cmp(&b.0));
            for (key, value) in self.facts {
                builder.push(key, value)?;
            }
        }
        let result = builder
            .finish()?
            .ok_or_else(|| storage("composed fragment has no fields"))?;
        if self.spilled {
            self.connection
                .prepare_cached("DELETE FROM request_source_fact_spool")
                .map_err(|error| storage(error.to_string()))?
                .execute([])
                .map_err(|error| storage(error.to_string()))?;
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn connection() -> Connection {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE request_source_pages(hash BLOB PRIMARY KEY,bytes BLOB NOT NULL)",
            )
            .unwrap();
        connection
    }
    #[test]
    fn small_sort_matches_independent_sql_binary_order_without_temp_table() {
        let memory = connection();
        let disk = connection();
        disk.execute_batch(
            "CREATE TEMP TABLE original(key TEXT PRIMARY KEY,value BLOB NOT NULL) WITHOUT ROWID",
        )
        .unwrap();
        let mut spool = FactSpool::new(&memory);
        for key in ["水", "a\0b", "A", "[\"escaped\\\"\"]", "a", "🙂"] {
            let value = key.as_bytes().to_vec();
            disk.execute("INSERT INTO original VALUES(?1,?2)", params![key, value])
                .unwrap();
            spool.push(key.into(), value).unwrap();
        }
        assert!(!spool.spilled);
        assert!(spool.bytes <= MAX_BYTES);
        let actual = spool.finish().unwrap();
        let mut expected = index::Builder::new(&disk);
        let mut statement = disk
            .prepare("SELECT key,value FROM original ORDER BY key")
            .unwrap();
        let mut rows = statement.query([]).unwrap();
        while let Some(row) = rows.next().unwrap() {
            expected
                .push(row.get(0).unwrap(), row.get(1).unwrap())
                .unwrap();
        }
        let expected = expected.finish().unwrap().unwrap();
        assert_eq!(actual.hash, expected.hash);
        assert_eq!(actual.last, expected.last);
        assert_eq!(actual.depth, expected.depth);
        assert_eq!(memory.query_row("SELECT count(*) FROM sqlite_temp_schema WHERE name='request_source_fact_spool'",[],|row|row.get::<_,i64>(0)).unwrap(),0);
    }
    #[test]
    fn spool_spills_on_count_and_owned_capacity_and_preserves_every_fact() {
        let connection = connection();
        let mut spool = FactSpool::new(&connection);
        for n in 0..MAX_FACTS {
            spool.push(format!("k{n:04}"), vec![1]).unwrap();
        }
        assert!(!spool.spilled);
        assert_eq!(spool.facts.len(), MAX_FACTS);
        assert!(spool.bytes <= MAX_BYTES);
        spool.push("last".into(), vec![2]).unwrap();
        assert!(spool.spilled);
        assert!(spool.facts.is_empty());
        let root = spool.finish().unwrap();
        for n in 0..MAX_FACTS {
            assert_eq!(
                index::get(&connection, &root, &format!("k{n:04}"))
                    .unwrap()
                    .unwrap(),
                vec![1]
            );
        }
        assert_eq!(
            index::get(&connection, &root, "last").unwrap().unwrap(),
            vec![2]
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT count(*) FROM request_source_fact_spool",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
        let mut spool = FactSpool::new(&connection);
        let mut value = Vec::with_capacity(MAX_BYTES);
        value.push(3);
        spool.push("capacity".into(), value).unwrap();
        assert!(spool.spilled);
        let root = spool.finish().unwrap();
        assert_eq!(
            index::get(&connection, &root, "capacity").unwrap().unwrap(),
            vec![3]
        );
    }
    #[test]
    fn spool_rejects_duplicates_and_observes_disk_errors() {
        let connection = connection();
        for disk in [false, true] {
            let mut spool = FactSpool::new(&connection);
            if disk {
                spool.spill().unwrap();
            }
            spool.push("same".into(), vec![1]).unwrap();
            assert!(spool.push("same".into(), vec![2]).is_err());
            let root = spool.finish().unwrap();
            assert_eq!(
                index::get(&connection, &root, "same").unwrap().unwrap(),
                vec![1]
            );
        }
        connection.execute_batch("DROP TABLE request_source_fact_spool; CREATE TEMP TABLE request_source_fact_spool(wrong TEXT)").unwrap();
        let mut spool = FactSpool::new(&connection);
        spool.push("first".into(), vec![1]).unwrap();
        assert!(spool.spill().is_err());
    }
}

#[cfg(test)]
#[test]
fn failed_capture_rolls_back_spilled_temp_facts_with_its_native_savepoint() {
    let mut connection = Connection::open_in_memory().unwrap();
    let transaction = connection.transaction().unwrap();
    transaction.execute_batch("SAVEPOINT capture").unwrap();
    {
        let mut spool = FactSpool::new(&transaction);
        spool.spill().unwrap();
        spool.push("first".into(), vec![1]).unwrap();
        assert!(spool.push("first".into(), vec![2]).is_err());
    }
    transaction
        .execute_batch("ROLLBACK TO capture; RELEASE capture")
        .unwrap();
    transaction.commit().unwrap();
    assert_eq!(
        connection
            .query_row(
                "SELECT count(*) FROM sqlite_temp_schema WHERE name='request_source_fact_spool'",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
}
