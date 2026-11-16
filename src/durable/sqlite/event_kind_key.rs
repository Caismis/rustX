//! Compact, typed keys for the common Journal lifecycle. External kinds remain text.
//! Codes follow commit groups, so final-settlement kinds share neighboring leaves.
use rusqlite::types::Value;

macro_rules! sql {
    ($value:literal) => {
        concat!(
            "(CASE ",
            $value,
            " WHEN 'inbound_turn_adopted' THEN x'0001'",
            " WHEN 'attempt_started' THEN x'0002'",
            " WHEN 'turn_started' THEN x'0003'",
            " WHEN 'model_request_started' THEN x'0004'",
            " WHEN 'model_request_completed' THEN x'0005'",
            " WHEN 'assistant_message_committed' THEN x'0006'",
            " WHEN 'turn_completed' THEN x'0007'",
            " WHEN 'attempt_completed' THEN x'0008'",
            // Arbitrary corrupt BLOBs must never alias an encoded text kind.
            " ELSE CASE WHEN typeof(",
            $value,
            ")='blob' THEN CAST(x'01'||",
            $value,
            " AS BLOB) ELSE ",
            $value,
            " END END)"
        )
    };
}
pub(super) use sql;

pub(super) const NATIVE: &str = sql!("event_kind");
pub(super) const OBSERVATION: &str = sql!("observation_kind");

pub(super) fn parameter(kind: &str) -> Value {
    let code = match kind {
        "inbound_turn_adopted" => 1,
        "attempt_started" => 2,
        "turn_started" => 3,
        "model_request_started" => 4,
        "model_request_completed" => 5,
        "assistant_message_committed" => 6,
        "turn_completed" => 7,
        "attempt_completed" => 8,
        _ => return Value::Text(kind.into()),
    };
    Value::Blob(vec![0, code])
}

#[cfg(test)]
mod tests;
