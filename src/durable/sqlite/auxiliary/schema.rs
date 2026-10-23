//! Auxiliary audit tables, indexes, and their creation and shape validation.

use super::{Connection, ConversationStoreError, OptionalExtension, params, storage};

pub(super) const AUXILIARY_SCHEMA: &str = r"
CREATE INDEX IF NOT EXISTS events_primary_trace_kind_idx
    ON events(observation_kind,sequence) WHERE observation_trace_fact_required=1;
CREATE TABLE IF NOT EXISTS request_owners (
    request_id TEXT PRIMARY KEY
        CHECK(length(CAST(request_id AS BLOB)) >= 1),
    owner_kind TEXT NOT NULL CHECK(owner_kind IN ('agent','auxiliary')),
    attempt_id TEXT,
    turn_id TEXT,
    retry_number INTEGER,
    call_id TEXT REFERENCES auxiliary_calls(call_id),
    auxiliary_ordinal INTEGER,
    start_sequence INTEGER NOT NULL UNIQUE REFERENCES events(sequence),
    terminal_sequence INTEGER UNIQUE REFERENCES events(sequence),
    terminal_kind TEXT CHECK(terminal_kind IN ('completed','failed')),
    CHECK(owner_kind='agent'
          OR length(CAST(request_id AS BLOB)) <= 512),
    CHECK(
        (owner_kind='agent'
         AND attempt_id IS NOT NULL AND turn_id IS NOT NULL
         AND retry_number IS NOT NULL
         AND retry_number BETWEEN 0 AND 4294967295
         AND call_id IS NULL AND auxiliary_ordinal IS NULL)
        OR
        (owner_kind='auxiliary'
         AND attempt_id IS NULL AND turn_id IS NULL AND retry_number IS NULL
         AND call_id IS NOT NULL
         AND auxiliary_ordinal IS NOT NULL
         AND auxiliary_ordinal BETWEEN 0 AND 2)
    ),
    CHECK((terminal_sequence IS NULL) = (terminal_kind IS NULL))
);

CREATE TABLE IF NOT EXISTS auxiliary_calls (
    call_id TEXT PRIMARY KEY
        CHECK(length(CAST(call_id AS BLOB)) BETWEEN 1 AND 64),
    started_sequence INTEGER NOT NULL UNIQUE REFERENCES events(sequence),
    settled_sequence INTEGER UNIQUE REFERENCES events(sequence),
    outcome TEXT CHECK(outcome IN ('completed','failed','cancelled')),
    CHECK((settled_sequence IS NULL) = (outcome IS NULL))
);

CREATE TABLE IF NOT EXISTS auxiliary_requests (
    call_id TEXT NOT NULL REFERENCES auxiliary_calls(call_id),
    ordinal INTEGER NOT NULL CHECK(ordinal BETWEEN 0 AND 2),
    request_id TEXT NOT NULL UNIQUE
        CHECK(length(CAST(request_id AS BLOB)) BETWEEN 1 AND 512),
    model TEXT NOT NULL CHECK(length(CAST(model AS BLOB)) BETWEEN 1 AND 512),
    started_sequence INTEGER NOT NULL UNIQUE REFERENCES events(sequence),
    terminal_sequence INTEGER UNIQUE REFERENCES events(sequence),
    terminal_kind TEXT CHECK(terminal_kind IN ('completed','failed')),
    PRIMARY KEY(call_id,ordinal),
    CHECK((terminal_sequence IS NULL) = (terminal_kind IS NULL))
);

CREATE UNIQUE INDEX IF NOT EXISTS auxiliary_requests_open_idx
    ON auxiliary_requests(call_id) WHERE terminal_sequence IS NULL;
CREATE INDEX IF NOT EXISTS auxiliary_requests_request_idx
    ON auxiliary_requests(request_id,started_sequence);
CREATE INDEX IF NOT EXISTS auxiliary_requests_terminal_idx
    ON auxiliary_requests(call_id,terminal_sequence);

CREATE TABLE IF NOT EXISTS auxiliary_retries (
    request_id TEXT PRIMARY KEY REFERENCES auxiliary_requests(request_id),
    call_id TEXT NOT NULL REFERENCES auxiliary_calls(call_id),
    next_ordinal INTEGER NOT NULL CHECK(next_ordinal BETWEEN 1 AND 2),
    sequence INTEGER NOT NULL UNIQUE REFERENCES events(sequence),
    UNIQUE(call_id,next_ordinal)
);
CREATE TABLE IF NOT EXISTS auxiliary_title_calls (
    call_id TEXT PRIMARY KEY REFERENCES auxiliary_calls(call_id),
    message_id TEXT NOT NULL REFERENCES message_ledger(message_id)
);
CREATE INDEX IF NOT EXISTS auxiliary_title_calls_source_idx ON auxiliary_title_calls(message_id,call_id);
CREATE TABLE IF NOT EXISTS auxiliary_title_sources (
    message_id TEXT PRIMARY KEY REFERENCES message_ledger(message_id),
    call_id TEXT UNIQUE REFERENCES auxiliary_calls(call_id),
    consumed INTEGER NOT NULL CHECK(consumed IN (0,1))
);
CREATE TABLE IF NOT EXISTS auxiliary_audit_facts (
    sequence INTEGER PRIMARY KEY REFERENCES events(sequence),
    audit_offset INTEGER NOT NULL CHECK(audit_offset >= 1),
    kind TEXT NOT NULL CHECK(kind IN (
        'call_started','request_started','request_settled','call_settled','retry_scheduled'
    )),
    call_id TEXT NOT NULL REFERENCES auxiliary_calls(call_id),
    request_id TEXT
        CHECK(request_id IS NULL OR length(CAST(request_id AS BLOB)) BETWEEN 1 AND 512),
    ordinal INTEGER,
    CHECK(
        (kind IN ('call_started','call_settled')
         AND request_id IS NULL AND ordinal IS NULL)
        OR
        (kind IN ('request_started','request_settled','retry_scheduled')
         AND request_id IS NOT NULL
         AND ordinal IS NOT NULL
         AND ordinal BETWEEN 0 AND 4294967295)
    )
);

CREATE UNIQUE INDEX IF NOT EXISTS auxiliary_audit_offset_idx
    ON auxiliary_audit_facts(audit_offset);
CREATE INDEX IF NOT EXISTS auxiliary_audit_call_idx
    ON auxiliary_audit_facts(call_id,sequence);
CREATE INDEX IF NOT EXISTS auxiliary_audit_request_idx
    ON auxiliary_audit_facts(request_id,sequence)
    WHERE request_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS auxiliary_audit_sequence_idx
    ON auxiliary_audit_facts(sequence,audit_offset);
CREATE INDEX IF NOT EXISTS auxiliary_calls_started_idx
    ON auxiliary_calls(started_sequence);
CREATE INDEX IF NOT EXISTS auxiliary_calls_settled_idx
    ON auxiliary_calls(settled_sequence)
    WHERE settled_sequence IS NOT NULL;
CREATE INDEX IF NOT EXISTS request_owners_source_idx
    ON request_owners(owner_kind,call_id,auxiliary_ordinal)
    WHERE owner_kind='auxiliary';
CREATE INDEX IF NOT EXISTS events_auxiliary_call_id_kind_idx
    ON events(json_extract(event_json,'$.event.call_id'),
              json_extract(event_json,'$.event.type'),sequence);
CREATE INDEX IF NOT EXISTS events_auxiliary_fact_type_idx
    ON events(json_extract(event_json,'$.event.type'),sequence)
    WHERE (
        json_extract(event_json,'$.event.type') IN
            ('model_request_started','model_request_completed','model_request_failed')
        AND json_extract(event_json,'$.event.owner.type')='auxiliary'
    )
    OR json_extract(event_json,'$.event.type') IN
        ('auxiliary_call_started','auxiliary_call_settled','auxiliary_retry_scheduled');
";

pub(super) const AUXILIARY_SEQUENCE_ENCODING_DIGITS_MAX: usize = 20;

pub(super) struct AuxiliaryIndexShape {
    pub(super) table: &'static str,
    pub(super) name: &'static str,
    pub(super) unique: bool,
    pub(super) partial: bool,
    pub(super) columns: &'static [Option<&'static str>],
    pub(super) definition: &'static str,
}

pub(super) const AUXILIARY_INDEXES: &[AuxiliaryIndexShape] = &[
    AuxiliaryIndexShape {
        table: "auxiliary_title_calls",
        name: "auxiliary_title_calls_source_idx",
        unique: false,
        partial: false,
        columns: &[Some("message_id"), Some("call_id")],
        definition: "CREATE INDEX auxiliary_title_calls_source_idx ON auxiliary_title_calls(message_id,call_id)",
    },
    AuxiliaryIndexShape {
        table: "message_ledger",
        name: "message_ledger_first_human_idx",
        unique: false,
        partial: true,
        columns: &[Some("position")],
        definition: "CREATE INDEX message_ledger_first_human_idx ON message_ledger(position) WHERE ordinary_human=1",
    },
    AuxiliaryIndexShape {
        table: "events",
        name: "events_primary_trace_kind_idx",
        unique: false,
        partial: true,
        columns: &[Some("observation_kind"), Some("sequence")],
        definition: "CREATE INDEX events_primary_trace_kind_idx ON events(observation_kind,sequence) WHERE observation_trace_fact_required=1",
    },
    AuxiliaryIndexShape {
        table: "auxiliary_requests",
        name: "auxiliary_requests_open_idx",
        unique: true,
        partial: true,
        columns: &[Some("call_id")],
        definition: "CREATE UNIQUE INDEX auxiliary_requests_open_idx ON auxiliary_requests(call_id) WHERE terminal_sequence IS NULL",
    },
    AuxiliaryIndexShape {
        table: "auxiliary_requests",
        name: "auxiliary_requests_request_idx",
        unique: false,
        partial: false,
        columns: &[Some("request_id"), Some("started_sequence")],
        definition: "CREATE INDEX auxiliary_requests_request_idx ON auxiliary_requests(request_id,started_sequence)",
    },
    AuxiliaryIndexShape {
        table: "auxiliary_requests",
        name: "auxiliary_requests_terminal_idx",
        unique: false,
        partial: false,
        columns: &[Some("call_id"), Some("terminal_sequence")],
        definition: "CREATE INDEX auxiliary_requests_terminal_idx ON auxiliary_requests(call_id,terminal_sequence)",
    },
    AuxiliaryIndexShape {
        table: "auxiliary_audit_facts",
        name: "auxiliary_audit_offset_idx",
        unique: true,
        partial: false,
        columns: &[Some("audit_offset")],
        definition: "CREATE UNIQUE INDEX auxiliary_audit_offset_idx ON auxiliary_audit_facts(audit_offset)",
    },
    AuxiliaryIndexShape {
        table: "auxiliary_audit_facts",
        name: "auxiliary_audit_call_idx",
        unique: false,
        partial: false,
        columns: &[Some("call_id"), Some("sequence")],
        definition: "CREATE INDEX auxiliary_audit_call_idx ON auxiliary_audit_facts(call_id,sequence)",
    },
    AuxiliaryIndexShape {
        table: "auxiliary_audit_facts",
        name: "auxiliary_audit_request_idx",
        unique: false,
        partial: true,
        columns: &[Some("request_id"), Some("sequence")],
        definition: "CREATE INDEX auxiliary_audit_request_idx ON auxiliary_audit_facts(request_id,sequence) WHERE request_id IS NOT NULL",
    },
    AuxiliaryIndexShape {
        table: "auxiliary_audit_facts",
        name: "auxiliary_audit_sequence_idx",
        unique: false,
        partial: false,
        columns: &[Some("sequence"), Some("audit_offset")],
        definition: "CREATE INDEX auxiliary_audit_sequence_idx ON auxiliary_audit_facts(sequence,audit_offset)",
    },
    AuxiliaryIndexShape {
        table: "auxiliary_calls",
        name: "auxiliary_calls_started_idx",
        unique: false,
        partial: false,
        columns: &[Some("started_sequence")],
        definition: "CREATE INDEX auxiliary_calls_started_idx ON auxiliary_calls(started_sequence)",
    },
    AuxiliaryIndexShape {
        table: "auxiliary_calls",
        name: "auxiliary_calls_settled_idx",
        unique: false,
        partial: true,
        columns: &[Some("settled_sequence")],
        definition: "CREATE INDEX auxiliary_calls_settled_idx ON auxiliary_calls(settled_sequence) WHERE settled_sequence IS NOT NULL",
    },
    AuxiliaryIndexShape {
        table: "request_owners",
        name: "request_owners_source_idx",
        unique: false,
        partial: true,
        columns: &[
            Some("owner_kind"),
            Some("call_id"),
            Some("auxiliary_ordinal"),
        ],
        definition: "CREATE INDEX request_owners_source_idx ON request_owners(owner_kind,call_id,auxiliary_ordinal) WHERE owner_kind='auxiliary'",
    },
    AuxiliaryIndexShape {
        table: "events",
        name: "events_auxiliary_call_id_kind_idx",
        unique: false,
        partial: false,
        columns: &[None, None, Some("sequence")],
        definition: "CREATE INDEX events_auxiliary_call_id_kind_idx ON events(json_extract(event_json,'$.event.call_id'),json_extract(event_json,'$.event.type'),sequence)",
    },
    AuxiliaryIndexShape {
        table: "events",
        name: "events_auxiliary_fact_type_idx",
        unique: false,
        partial: true,
        columns: &[None, Some("sequence")],
        definition: "CREATE INDEX events_auxiliary_fact_type_idx ON events(json_extract(event_json,'$.event.type'),sequence) WHERE (json_extract(event_json,'$.event.type') IN ('model_request_started','model_request_completed','model_request_failed') AND json_extract(event_json,'$.event.owner.type')='auxiliary') OR json_extract(event_json,'$.event.type') IN ('auxiliary_call_started','auxiliary_call_settled','auxiliary_retry_scheduled')",
    },
];

pub(in crate::durable::sqlite) fn create_schema(
    connection: &Connection,
) -> Result<(), ConversationStoreError> {
    connection
        .execute_batch(AUXILIARY_SCHEMA)
        .map_err(|error| storage(format!("create auxiliary owner schema: {error}")))
}

pub(super) fn normalized_index_definition(value: &str) -> String {
    value
        .chars()
        .filter(|character| !character.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect::<String>()
        .replace("ifnotexists", "")
}

pub(super) fn verify_required_indexes(
    connection: &Connection,
) -> Result<(), ConversationStoreError> {
    for expected in AUXILIARY_INDEXES {
        let listed: Option<(bool, bool)> = connection
            .prepare(&format!("PRAGMA index_list({})", expected.table))
            .map_err(|error| {
                storage(format!(
                    "inspect auxiliary index {} on {}: {error}",
                    expected.name, expected.table
                ))
            })?
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(4)?,
                ))
            })
            .map_err(|error| storage(format!("read auxiliary indexes: {error}")))?
            .find_map(|row| match row {
                Ok((name, unique, partial)) if name == expected.name => {
                    Some(Ok((unique == 1, partial == 1)))
                }
                Ok(_) => None,
                Err(error) => Some(Err(error)),
            })
            .transpose()
            .map_err(|error| storage(format!("read auxiliary index: {error}")))?;
        let Some((unique, partial)) = listed else {
            return Err(ConversationStoreError::IncompatibleSchema(format!(
                "missing required auxiliary index {}",
                expected.name
            )));
        };
        if unique != expected.unique || partial != expected.partial {
            return Err(ConversationStoreError::IncompatibleSchema(format!(
                "auxiliary index {} has the wrong uniqueness or partial shape",
                expected.name
            )));
        }
        let quoted = expected.name.replace('"', "\"\"");
        let columns = connection
            .prepare(&format!("PRAGMA index_info(\"{quoted}\")"))
            .map_err(|error| storage(format!("inspect auxiliary index columns: {error}")))?
            .query_map([], |row| row.get::<_, Option<String>>(2))
            .map_err(|error| storage(format!("read auxiliary index columns: {error}")))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| storage(format!("collect auxiliary index columns: {error}")))?;
        if columns.len() != expected.columns.len()
            || columns
                .iter()
                .zip(expected.columns)
                .any(|(actual, expected)| actual.as_deref() != *expected)
        {
            return Err(ConversationStoreError::IncompatibleSchema(format!(
                "auxiliary index {} has the wrong columns",
                expected.name
            )));
        }
        let definition: Option<String> = connection
            .query_row(
                "SELECT sql FROM sqlite_schema
                 WHERE type='index' AND name=?1 AND tbl_name=?2",
                params![expected.name, expected.table],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| storage(format!("read auxiliary index definition: {error}")))?;
        let expected_definition = normalized_index_definition(expected.definition);
        if definition
            .as_deref()
            .map(normalized_index_definition)
            .as_deref()
            != Some(expected_definition.as_str())
        {
            return Err(ConversationStoreError::IncompatibleSchema(format!(
                "auxiliary index {} has the wrong column or predicate definition",
                expected.name
            )));
        }
    }
    Ok(())
}

pub(in crate::durable::sqlite) fn verify_schema(
    connection: &Connection,
) -> Result<(), ConversationStoreError> {
    for (table, columns) in [
        (
            "auxiliary_retries",
            &["request_id", "call_id", "next_ordinal", "sequence"][..],
        ),
        (
            "auxiliary_title_sources",
            &["message_id", "call_id", "consumed"][..],
        ),
        ("auxiliary_title_calls", &["call_id", "message_id"][..]),
        (
            "request_owners",
            &[
                "request_id",
                "owner_kind",
                "attempt_id",
                "turn_id",
                "retry_number",
                "call_id",
                "auxiliary_ordinal",
                "start_sequence",
                "terminal_sequence",
                "terminal_kind",
            ][..],
        ),
        (
            "auxiliary_calls",
            &["call_id", "started_sequence", "settled_sequence", "outcome"][..],
        ),
        (
            "auxiliary_requests",
            &[
                "call_id",
                "ordinal",
                "request_id",
                "model",
                "started_sequence",
                "terminal_sequence",
                "terminal_kind",
            ][..],
        ),
        (
            "auxiliary_audit_facts",
            &[
                "sequence",
                "audit_offset",
                "kind",
                "call_id",
                "request_id",
                "ordinal",
            ][..],
        ),
    ] {
        let mut statement = connection
            .prepare(&format!("PRAGMA table_info({table})"))
            .map_err(|error| storage(format!("inspect auxiliary table {table}: {error}")))?;
        let actual = statement
            .query_map([], |row| row.get::<_, String>(1))
            .map_err(|error| storage(format!("read auxiliary table {table}: {error}")))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| storage(format!("collect auxiliary table {table}: {error}")))?;
        for column in columns {
            if !actual.iter().any(|candidate| candidate == column) {
                return Err(ConversationStoreError::IncompatibleSchema(format!(
                    "table {table} is missing required column {column}"
                )));
            }
        }
        if table == "auxiliary_calls"
            && actual.iter().any(|column| {
                matches!(
                    column.as_str(),
                    "purpose"
                        | "source"
                        | "trigger"
                        | "business_source"
                        | "input"
                        | "semantic_input_fingerprint"
                        | "invocation_fingerprint"
                        | "history_cut"
                )
            })
        {
            return Err(ConversationStoreError::IncompatibleSchema(
                "auxiliary_calls contains a retired metadata layout".into(),
            ));
        }
    }
    verify_required_indexes(connection)?;
    for (table, column) in [
        ("request_owners", "request_id"),
        ("request_owners", "start_sequence"),
        ("request_owners", "terminal_sequence"),
        ("auxiliary_calls", "call_id"),
        ("auxiliary_calls", "started_sequence"),
        ("auxiliary_calls", "settled_sequence"),
        ("auxiliary_requests", "request_id"),
        ("auxiliary_requests", "started_sequence"),
        ("auxiliary_requests", "terminal_sequence"),
        ("auxiliary_audit_facts", "audit_offset"),
    ] {
        crate::durable::sqlite::schema::shape::verify_unique_column(connection, table, column)?;
    }
    crate::durable::sqlite::schema::shape::verify_unique_columns(
        connection,
        "auxiliary_requests",
        &["call_id", "ordinal"],
    )?;
    Ok(())
}
