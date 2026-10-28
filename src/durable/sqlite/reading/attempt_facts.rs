//! Chronological folds merge a fixed set of kind seeks from the shared index.
//! SQLite's UNION ALL merge keeps one current row per branch, so these readers
//! need neither a second attempt index nor an in-memory sort of the history.
use std::sync::LazyLock;

const EXECUTION_KINDS: &[&str] = &[
    "turn_started",
    "model_request_started",
    "model_request_completed",
    "model_request_failed",
    "tool_execution_started",
    "tool_execution_completed",
    "tool_execution_failed",
    "compaction_started",
    "compaction_completed",
    "assistant_message_committed",
    "attempt_completed",
];
const LINEAGE_KINDS: &[&str] = &[
    "assistant_message_committed",
    "attempt_completed",
    "attempt_cancelled",
    "attempt_failed",
    "attempt_timed_out",
    "attempt_limit_exceeded",
];

pub(super) static EXECUTION: LazyLock<String> = LazyLock::new(|| ordered(EXECUTION_KINDS));
pub(super) static LINEAGE: LazyLock<String> = LazyLock::new(|| ordered(LINEAGE_KINDS));

fn ordered(kinds: &[&str]) -> String {
    // Only the closed literal kind lists above enter SQL. ?1 and ?2 are shared
    // by every branch, including nullable/mismatched native owner cases.
    let branches = kinds
        .iter()
        .map(|kind| {
            format!(
                "SELECT event_json,sequence FROM events INDEXED BY events_attempt_kind_idx \
             WHERE attempt_id=?1 AND sequence<=?2 \
             AND json_extract(event_json,'$.event.type')='{kind}'"
            )
        })
        .collect::<Vec<_>>();
    format!("{} ORDER BY sequence", branches.join(" UNION ALL "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::durable::SqliteConversationStore;
    use crate::runtime::identity::ConversationId;
    use rusqlite::{Connection, StatementStatus, params};

    fn rows(
        connection: &Connection,
        sql: &str,
        owner: Option<&str>,
        through: i64,
    ) -> Vec<(String, i64)> {
        let mut statement = connection.prepare(sql).unwrap();
        statement
            .query_map(params![owner, through], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    }

    #[test]
    fn merged_attempt_folds_preserve_native_selection_order_and_every_history_cut() {
        let store = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
        let connection = store.lock().unwrap();
        for sequence in 1..=36 {
            let kind = EXECUTION_KINDS[(sequence * 7) % EXECUTION_KINDS.len()];
            let owner = match sequence % 5 {
                0 => None,
                1 => Some("other"),
                _ => Some("selected"),
            };
            // Deliberately incomplete native envelopes must reach the caller's
            // decoder in exactly the same order instead of being filtered out.
            let json =
                serde_json::json!({"event":{"type":kind},"attempt_id":"different"}).to_string();
            connection.execute("INSERT INTO events(sequence,event_id,schema_version,conversation_id,event_timestamp,attempt_id,event_json) VALUES(?1,?2,1,'conversation','time',?3,?4)", params![sequence, format!("event-{sequence}"), owner, json]).unwrap();
        }
        for (sql, kinds) in [(&**EXECUTION, EXECUTION_KINDS), (&**LINEAGE, LINEAGE_KINDS)] {
            let oracle = format!(
                "SELECT event_json,sequence FROM events NOT INDEXED WHERE attempt_id=?1 AND sequence<=?2 AND json_extract(event_json,'$.event.type') IN ('{}') ORDER BY sequence",
                kinds.join("','")
            );
            for through in 0..=37 {
                for owner in [None, Some("selected"), Some("other"), Some("absent")] {
                    assert_eq!(
                        rows(&connection, sql, owner, through),
                        rows(&connection, &oracle, owner, through)
                    );
                }
            }
            connection.execute("UPDATE events SET event_json=json_set(event_json,'$.event.type','attempt_failed') WHERE sequence=7", []).unwrap();
            assert_eq!(
                rows(&connection, sql, Some("selected"), 36),
                rows(&connection, &oracle, Some("selected"), 36)
            );
        }
    }

    #[test]
    fn merged_attempt_folds_seek_each_kind_without_history_sort_or_secondary_attempt_index() {
        let store = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
        let connection = store.lock().unwrap();
        assert!(
            !connection
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name='events_attempt_idx')",
                    [],
                    |row| row.get::<_, bool>(0)
                )
                .unwrap()
        );
        connection.execute_batch("WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<4096) INSERT INTO events(sequence,event_id,schema_version,conversation_id,event_timestamp,attempt_id,event_json) SELECT x,'event-'||x,1,'conversation','time','selected','{\"event\":{\"type\":\"unrelated\"}}' FROM n").unwrap();
        for (sequence, kind) in [
            (4097, "assistant_message_committed"),
            (4098, "attempt_completed"),
            (4099, "assistant_message_committed"),
        ] {
            connection.execute("INSERT INTO events(sequence,event_id,schema_version,conversation_id,event_timestamp,attempt_id,event_json) VALUES(?1,?2,1,'conversation','time','selected',?3)", params![sequence, format!("event-{sequence}"), serde_json::json!({"event":{"type":kind}}).to_string()]).unwrap();
        }
        for (sql, kinds) in [(&**EXECUTION, EXECUTION_KINDS), (&**LINEAGE, LINEAGE_KINDS)] {
            let plan = connection
                .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
                .unwrap()
                .query_map(params!["selected", 4099], |row| row.get::<_, String>(3))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            assert_eq!(
                plan.iter()
                    .filter(|line| line
                        .contains("SEARCH events USING INDEX events_attempt_kind_idx")
                        && line.contains("attempt_id=?")
                        && line.contains("sequence<?"))
                    .count(),
                kinds.len(),
                "{plan:?}"
            );
            assert!(
                plan.iter().any(|line| line.contains("MERGE (UNION ALL)")),
                "{plan:?}"
            );
            assert!(
                !plan
                    .iter()
                    .any(|line| line.contains("SCAN") || line.contains("TEMP B-TREE")),
                "{plan:?}"
            );
            let mut statement = connection.prepare(sql).unwrap();
            assert_eq!(
                statement
                    .query_map(params!["selected", 4099], |row| row.get::<_, i64>(1))
                    .unwrap()
                    .collect::<Result<Vec<_>, _>>()
                    .unwrap(),
                [4097, 4098, 4099]
            );
            assert_eq!(statement.get_status(StatementStatus::FullscanStep), 0);
            assert_eq!(statement.get_status(StatementStatus::Sort), 0);
            assert!(statement.get_status(StatementStatus::VmStep) < 500);
        }
    }
}
