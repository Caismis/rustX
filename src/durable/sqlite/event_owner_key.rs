//! Lossless numeric keys for the runtime's existing owner spelling.
//! Exact round trips alone become integers. Other producer identities keep
//! their original SQLite type in generated columns without affinity, so text
//! aliases cannot collide with an encoded integer. No mutable dictionary or
//! connection-specific SQL function is needed.

macro_rules! attempt {
    ($value:literal, $conversation:literal) => {
        concat!(
            "CASE WHEN ",
            $value,
            "=",
            $conversation,
            "||'-attempt-'||CAST(CAST(substr(",
            $value,
            ",length(",
            $conversation,
            ")+10) AS INTEGER) AS TEXT) THEN CAST(substr(",
            $value,
            ",length(",
            $conversation,
            ")+10) AS INTEGER) ELSE ",
            $value,
            " END"
        )
    };
}
pub(super) use attempt;

macro_rules! turn {
    ($value:literal) => {
        concat!(
            "CASE WHEN ",
            $value,
            "=CAST(CAST(",
            $value,
            " AS INTEGER) AS TEXT) THEN CAST(",
            $value,
            " AS INTEGER) ELSE ",
            $value,
            " END"
        )
    };
}
pub(super) use turn;

macro_rules! attempt_parameter {
    ($value:literal) => {
        concat!(
            "(SELECT ",
            crate::durable::sqlite::event_owner_key::attempt!($value, "conversation_id"),
            " FROM rustx_store WHERE id=1)"
        )
    };
}
pub(super) use attempt_parameter;
