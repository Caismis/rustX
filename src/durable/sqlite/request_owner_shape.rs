//! Indexed format-shape checks: opening a Store never walks all request bodies.
use super::{Connection, ConversationStoreError, OptionalExtension, storage};

const NAME: &str = "events_invalid_request_owner_idx";
// json_quote preserves JSON objects and safely quotes scalar owner corruption.
// Removing exactly the allowed keys avoids subqueries in a partial-index predicate.
const INVALID: &str = r"json_extract(event_json,'$.event.type') IN
 ('model_request_started','model_request_completed','model_request_failed')
 AND NOT COALESCE((
 json_type(event_json,'$.event.owner')='object' AND (
  (json_extract(event_json,'$.event.owner.type')='agent'
   AND json_type(event_json,'$.event.owner.identity')='object'
   AND json_remove(json_quote(json_extract(event_json,'$.event.owner')),'$.type','$.identity')='{}'
   AND json_remove(json_quote(json_extract(event_json,'$.event.owner.identity')),'$.attempt_id','$.turn','$.retry_number')='{}'
   AND json_type(event_json,'$.event.owner.identity.attempt_id')='text'
   AND json_type(event_json,'$.event.owner.identity.turn')='text'
   AND json_type(event_json,'$.event.owner.identity.retry_number')='integer'
   AND json_extract(event_json,'$.event.owner.identity.retry_number') BETWEEN 0 AND 4294967295)
  OR
  (json_extract(event_json,'$.event.owner.type')='auxiliary'
   AND json_remove(json_quote(json_extract(event_json,'$.event.owner')),'$.type','$.call_id','$.ordinal')='{}'
   AND json_type(event_json,'$.event.owner.call_id')='text'
   AND json_type(event_json,'$.event.owner.ordinal')='integer'
   AND json_extract(event_json,'$.event.owner.ordinal') BETWEEN 0 AND 4294967295)
  )),0)";

fn definition() -> String {
    format!("CREATE INDEX {NAME} ON events(sequence) WHERE {INVALID}")
}

pub(super) fn create_schema(connection: &Connection) -> Result<(), ConversationStoreError> {
    connection
        .execute_batch(&definition())
        .map_err(|error| storage(format!("create request owner shape index: {error}")))
}

fn query() -> String {
    format!("SELECT EXISTS(SELECT 1 FROM events INDEXED BY {NAME} WHERE {INVALID} LIMIT 1)")
}

fn normalized(sql: &str) -> String {
    sql.chars()
        .filter(|character| !character.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect()
}

pub(super) fn verify(connection: &Connection) -> Result<(), ConversationStoreError> {
    let definition: Option<Option<String>> = connection.query_row(
        "SELECT CASE WHEN octet_length(sql)<=8192 THEN sql END FROM sqlite_schema WHERE type='index' AND name=?1",
        [NAME], |row| row.get(0),
    ).optional().map_err(|error| storage(format!("request owner index schema: {error}")))?;
    let Some(definition) = definition.flatten() else {
        return Err(ConversationStoreError::IncompatibleSchema(
            "missing request owner shape index".into(),
        ));
    };
    if normalized(&definition) != normalized(&self::definition()) {
        return Err(ConversationStoreError::IncompatibleSchema(
            "invalid request owner shape index".into(),
        ));
    }
    let invalid: bool = connection
        .query_row(&query(), [], |row| row.get(0))
        .map_err(|error| storage(format!("request owner shape lookup: {error}")))?;
    if invalid {
        return Err(ConversationStoreError::IncompatibleSchema(
            "request facts require explicit typed ownership".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::StatementStatus;

    #[test]
    fn auxiliary_owner_shape_probe_is_constant_and_rejects_unknown_fields() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE events(sequence INTEGER PRIMARY KEY,event_json TEXT NOT NULL)",
            )
            .unwrap();
        create_schema(&connection).unwrap();
        let valid = r#"{"event":{"type":"model_request_completed","owner":{"type":"agent","identity":{"attempt_id":"a","turn":"1","retry_number":0}}}}"#;
        let steps = |connection: &Connection| {
            let mut query = connection.prepare(&query()).unwrap();
            assert!(!query.query_row([], |row| row.get::<_, bool>(0)).unwrap());
            query.get_status(StatementStatus::VmStep)
        };
        connection
            .execute("INSERT INTO events VALUES(1,?1)", [valid])
            .unwrap();
        let small = steps(&connection);
        for sequence in 2..=2000 {
            connection
                .execute(
                    "INSERT INTO events VALUES(?1,?2)",
                    rusqlite::params![sequence, valid],
                )
                .unwrap();
        }
        assert_eq!(steps(&connection), small);
        verify(&connection).unwrap();
        connection.execute("UPDATE events SET event_json=json_set(event_json,'$.event.owner.extra',1) WHERE sequence=2000", []).unwrap();
        assert!(matches!(
            verify(&connection),
            Err(ConversationStoreError::IncompatibleSchema(_))
        ));
        connection
            .execute(
                "UPDATE events SET event_json=?1 WHERE sequence=2000",
                [valid],
            )
            .unwrap();
        verify(&connection).unwrap();
        for owner in [
            serde_json::json!(null),
            serde_json::json!("not-an-owner"),
            serde_json::json!([]),
        ] {
            let mut value: serde_json::Value = serde_json::from_str(valid).unwrap();
            value["event"]["owner"] = owner;
            connection
                .execute(
                    "UPDATE events SET event_json=?1 WHERE sequence=2000",
                    [value.to_string()],
                )
                .unwrap();
            assert!(matches!(
                verify(&connection),
                Err(ConversationStoreError::IncompatibleSchema(_))
            ));
        }
        connection
            .execute_batch("DROP INDEX events_invalid_request_owner_idx")
            .unwrap();
        assert!(matches!(
            verify(&connection),
            Err(ConversationStoreError::IncompatibleSchema(_))
        ));
    }
}
