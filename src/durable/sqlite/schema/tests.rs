//! Schema open and validation regressions of the SQLite store.

use crate::durable::ConversationStoreError;
use crate::durable::sqlite::schema::shape::*;
use crate::durable::sqlite::schema::*;
use crate::durable::sqlite::test_support::*;
use crate::durable::sqlite::*;
use crate::durable::{CanonicalStore, InboxStore, JournalStore};
use crate::events::types::{EVENT_SCHEMA_VERSION, RuntimeEvent, RuntimeEventEnvelope};
use crate::runtime::identity::{AttemptId, ConversationId, TurnId};
use rusqlite::{Connection, params};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

#[test]
fn wal_management_head_is_one_snapshot_while_another_writer_commits() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("conversation.sqlite");
    let id = ConversationId::generate();
    let writer = Arc::new(SqliteConversationStore::open(id.clone(), &path).unwrap());
    let reader = SqliteConversationStore::open_existing(id, &path).unwrap();
    let initial = reader.load_head().unwrap();
    let observed = Arc::new(AtomicBool::new(false));
    let committed = observed.clone();
    let competing = writer.clone();
    *reader.head_read_probe.lock().unwrap() = Some(Box::new(move || {
        let accepted = competing
            .accept_inbound(draft("during read snapshot"))
            .unwrap();
        competing
            .adopt_pending_batch(accepted.sequence, None)
            .unwrap();
        committed.store(true, Ordering::SeqCst);
    }));
    assert_eq!(reader.load_head().unwrap(), initial);
    assert!(
        observed.load(Ordering::SeqCst),
        "the interleaving gate must execute"
    );
    assert_eq!(reader.load_head().unwrap(), writer.load_head().unwrap());
    assert_eq!(reader.load_head().unwrap().active_message_ids.len(), 1);
}

#[test]
fn a_version_32_profile_store_is_refused_without_migration() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("profile-v33.sqlite");
    let id = ConversationId::new("conv_bb2149cf-1f8d-7c3c-8c60-73f4acb0a518");
    {
        let store = SqliteConversationStore::open(id.clone(), &path).unwrap();
        store
            .conn
            .lock()
            .unwrap()
            .execute(
                "UPDATE rustx_store SET schema_version = 32 WHERE id = 1",
                [],
            )
            .unwrap();
    }
    for result in [
        SqliteConversationStore::open(id.clone(), &path),
        SqliteConversationStore::open_existing(id, &path),
    ] {
        assert!(matches!(
            result,
            Err(ConversationStoreError::SchemaVersionMismatch {
                stored: 32,
                expected: SQLITE_SCHEMA_VERSION
            })
        ));
    }
    let connection = rusqlite::Connection::open(&path).unwrap();
    let stored: i64 = connection
        .query_row(
            "SELECT schema_version FROM rustx_store WHERE id = 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        stored, 32,
        "neither runtime nor management opening migrates"
    );
}

#[test]
fn reopening_a_store_for_the_wrong_conversation_fails_closed() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("conversation.sqlite");
    let conversation_id = ConversationId::new("conv_30d36aa9-aeae-79e9-8322-f17c7d13842a");
    drop(SqliteConversationStore::open(conversation_id, &path).unwrap());

    assert!(matches!(
        SqliteConversationStore::open(ConversationId::new("conv_ddb81244-a13c-7d38-842c-03c613793b90"), &path),
        Err(ConversationStoreError::ConversationIdMismatch { stored, requested })
            if stored == ConversationId::new("conv_30d36aa9-aeae-79e9-8322-f17c7d13842a")
                && requested == ConversationId::new("conv_ddb81244-a13c-7d38-842c-03c613793b90")
    ));
}

#[test]
fn new_schema_birth_rolls_back_when_shape_validation_fails() {
    use rusqlite::hooks::{AuthAction, AuthContext, Authorization};

    let mut connection = Connection::open_in_memory().unwrap();
    configure_connection(&mut connection, true).unwrap();
    let verification = Arc::new(AtomicBool::new(false));
    let observed = verification.clone();
    connection
        .authorizer(Some(move |context: AuthContext<'_>| {
            if matches!(
                context.action,
                AuthAction::Pragma {
                    pragma_name: "table_info",
                    pragma_value: Some("inherited_turn_provenance")
                }
            ) {
                observed.store(true, Ordering::SeqCst);
                Authorization::Deny
            } else {
                Authorization::Allow
            }
        }))
        .unwrap();
    assert!(create_schema(&mut connection).is_err());
    assert!(
        verification.load(Ordering::SeqCst),
        "the failure occurred during final shape validation"
    );
    connection
        .authorizer(None::<fn(AuthContext<'_>) -> Authorization>)
        .unwrap();
    assert!(connection.is_autocommit());
    let objects: u64 = connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        objects, 0,
        "validation failure rolls back every new table, index and root row"
    );
    create_schema(&mut connection).unwrap();
    verify_schema_shape(&connection).unwrap();
}

#[test]
fn incompatible_schema_fails_without_migration_or_repair() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("incompatible.sqlite");
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE rustx_store (id INTEGER PRIMARY KEY, schema_version INTEGER NOT NULL, conversation_id TEXT NOT NULL, next_inbound_sequence INTEGER NOT NULL, next_event_sequence INTEGER NOT NULL); INSERT INTO rustx_store VALUES (1, 99, 'conv-1', 0, 0);",
        )
        .unwrap();
    drop(connection);
    assert!(matches!(
        SqliteConversationStore::open(
            ConversationId::new("conv_36524fd8-f674-7fc2-8125-06d01fee0e18"),
            &path
        ),
        Err(ConversationStoreError::SchemaVersionMismatch {
            stored: 99,
            expected: SQLITE_SCHEMA_VERSION
        })
    ));
}

/// Issue #12 (M9b): a database written by the pre-M9b development schema
/// (version 1, whose `RequestSnapshot` JSON lacks the required
/// `request_context_ids` field) is rejected explicitly at store open with
/// a typed `SchemaVersionMismatch` — never a later accidental JSON decode
/// failure — and there is no migration, legacy reader, or compatibility
/// mode.
#[test]
fn pre_m9b_schema_version_is_rejected_explicitly() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("pre-m9b.sqlite");
    let conversation_id = ConversationId::new("conv_22848931-dd71-7669-86f8-0840ad55e434");
    {
        // Build a fully-shaped store at the current schema, then downgrade
        // only the schema version to the pre-M9b value: the table shape is
        // unchanged, but the serialized `RequestSnapshot` format gained a
        // required field, which is exactly why the version must gate open.
        let store = SqliteConversationStore::open(conversation_id.clone(), &path).unwrap();
        store
            .conn
            .lock()
            .unwrap()
            .execute("UPDATE rustx_store SET schema_version = 1 WHERE id = 1", [])
            .unwrap();
    }
    assert!(matches!(
        SqliteConversationStore::open(conversation_id, &path),
        Err(ConversationStoreError::SchemaVersionMismatch {
            stored: 1,
            expected: SQLITE_SCHEMA_VERSION
        })
    ));
}

/// Issue #242 changed the durable *semantic vocabulary* of the
/// Questionnaire interaction audit, so the store version must gate open.
///
/// A version-30 journal can hold the obsolete choice-only payload above.
/// Under the typed vocabulary an authored label is not an option index and
/// a request has no requester identity to recover, so there is nothing a
/// v31 reader could honestly make of those rows. The contract is
///
/// ```text
/// incompatible durable semantic format -> store-open rejection
/// ```
///
/// and never "open, then fail later while decoding an old record". This
/// proves both halves: `open` refuses the file, and the rows it refused
/// really are undecodable under the current vocabulary — so the gate is
/// what keeps the typed decoder from ever seeing them.
#[test]
fn a_version_30_questionnaire_journal_is_refused_at_open_never_decoded() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("questionnaire-v31.sqlite");
    let conversation_id = ConversationId::new("conv_7ee23a93-1fb2-7d39-84d8-def51c2d5f73");
    {
        // A fully-shaped store at the current schema, carrying the exact
        // rows a schema-30 runtime would have written, then downgraded to
        // the version that actually wrote them. The table shape is
        // unchanged — only the meaning of `event_json` moved, which is
        // precisely the class of change the version exists to gate.
        let store = SqliteConversationStore::open(conversation_id.clone(), &path).unwrap();
        let connection = store.conn.lock().unwrap();
        for (sequence, event_id, payload) in [
            (
                1i64,
                "interaction-requested-v31",
                V30_QUESTIONNAIRE_REQUESTED,
            ),
            (2i64, "interaction-settled-v31", V30_QUESTIONNAIRE_SETTLED),
        ] {
            connection
                .execute(
                    "INSERT INTO events(sequence,event_id,schema_version,conversation_id,attempt_id,turn_id,event_json,event_timestamp) VALUES(?1,?2,?3,?4,NULL,NULL,?5,json_extract(?5,'$.timestamp'))",
                    params![
                        sequence,
                        event_id,
                        i64::from(EVENT_SCHEMA_VERSION),
                        conversation_id.as_str(),
                        payload
                    ],
                )
                .unwrap();
        }
        connection
            .execute(
                "UPDATE rustx_store SET schema_version = 30, next_event_sequence = 2 WHERE id = 1",
                [],
            )
            .unwrap();
    }

    // The gate refuses the file, by version, at `open`. No store handle
    // exists afterwards, so no Event Journal read of those rows is
    // reachable at all.
    assert!(matches!(
        SqliteConversationStore::open(conversation_id, &path),
        Err(ConversationStoreError::SchemaVersionMismatch {
            stored: 30,
            expected: SQLITE_SCHEMA_VERSION
        })
    ));
    assert_eq!(SQLITE_SCHEMA_VERSION, 81);

    // And the refusal is not ceremony: had the gate admitted the file,
    // these are the rows the typed decoder would have had to interpret,
    // and it cannot. The envelope framing is unchanged (still
    // `EVENT_SCHEMA_VERSION`), which is exactly why the *store* version —
    // not the envelope version — is the one that had to move.
    for payload in [V30_QUESTIONNAIRE_REQUESTED, V30_QUESTIONNAIRE_SETTLED] {
        let framing: serde_json::Value = serde_json::from_str(payload).unwrap();
        assert_eq!(framing["schema_version"], i64::from(EVENT_SCHEMA_VERSION));
        assert!(
            serde_json::from_str::<RuntimeEventEnvelope>(payload).is_err(),
            "the typed vocabulary cannot interpret a schema-30 Questionnaire payload: {payload}"
        );
    }
}

/// Issue #258 made the effective execution-profile identity a durable
/// execution fact, so the store version must gate open.
///
/// A version-31 journal holds ownership facts with no `profile_digest`.
/// The one thing a v33 reader must never do is reconstruct it — the
/// current role definition and the current resource generation are both
/// mutable, and deriving identity from them is exactly what committing
/// the value durably exists to prevent. There is therefore nothing honest
/// to make of those rows, and the contract is the usual one:
///
/// ```text
/// incompatible durable semantic format -> store-open rejection
/// ```
#[test]
fn a_version_31_subagent_ownership_journal_is_refused_at_open_never_decoded() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("ownership-v31.sqlite");
    let conversation_id = ConversationId::new("conv_5b135cc1-effd-7c32-88a8-1e18c1b056bd");
    {
        let store = SqliteConversationStore::open(conversation_id.clone(), &path).unwrap();
        let connection = store.conn.lock().unwrap();
        connection
            .execute(
                "INSERT INTO events(sequence,event_id,schema_version,conversation_id,attempt_id,turn_id,event_json,event_timestamp) VALUES(?1,?2,?3,?4,NULL,NULL,?5,json_extract(?5,'$.timestamp'))",
                params![
                    1i64,
                    "subagent-committed-event:conv-v31-subagent-1",
                    i64::from(EVENT_SCHEMA_VERSION),
                    conversation_id.as_str(),
                    V31_SUBAGENT_OWNERSHIP
                ],
            )
            .unwrap();
        connection
            .execute(
                "UPDATE rustx_store SET schema_version = 31, next_event_sequence = 1 WHERE id = 1",
                [],
            )
            .unwrap();
    }

    assert!(matches!(
        SqliteConversationStore::open(conversation_id, &path),
        Err(ConversationStoreError::SchemaVersionMismatch {
            stored: 31,
            expected: SQLITE_SCHEMA_VERSION
        })
    ));
    assert_eq!(SQLITE_SCHEMA_VERSION, 81);

    // And the refusal is not ceremony: the envelope framing is unchanged,
    // and the row the gate refused really is undecodable under the current
    // vocabulary — which is exactly why the *store* version had to move.
    let framing: serde_json::Value = serde_json::from_str(V31_SUBAGENT_OWNERSHIP).unwrap();
    assert_eq!(framing["schema_version"], i64::from(EVENT_SCHEMA_VERSION));
    assert!(
        framing["event"].get("profile_digest").is_none(),
        "the schema-31 fact really does carry no effective profile identity"
    );
    assert!(
        serde_json::from_str::<RuntimeEventEnvelope>(V31_SUBAGENT_OWNERSHIP).is_err(),
        "the typed vocabulary cannot interpret a schema-31 ownership fact"
    );
}

#[test]
fn trace_schema_indexes_are_required_and_queries_use_them() {
    use crate::durable::presentation::{FactQuery, FactScope};
    let store = SqliteConversationStore::in_memory(ConversationId::new(
        "conv_ab18a567-5190-73a5-8156-2e8693be6392",
    ))
    .unwrap();
    let connection = store.conn.lock().unwrap();
    verify_schema_shape(&connection).unwrap();
    let scopes = [
        (FactScope::All, "events_kind_idx"),
        (FactScope::AgentRequest, "events_primary_request_kind_idx"),
        (
            FactScope::Attempt(AttemptId::new("a")),
            "events_attempt_kind_idx",
        ),
        (
            FactScope::Step(AttemptId::new("a"), TurnId::new("1")),
            "events_step_kind_idx",
        ),
        (FactScope::Request("r".into()), "events_request_id_kind_idx"),
        (FactScope::Message("m".into()), "events_message_id_kind_idx"),
        (FactScope::ContextReading, "events_context_reading_idx"),
        (
            FactScope::ToolCall {
                call_id: "c".into(),
                attempt: Some(AttemptId::new("a")),
                turn: Some(TurnId::new("1")),
            },
            "events_tool_call_scope_kind_idx",
        ),
        (
            FactScope::Execution("e".into()),
            "events_execution_id_kind_idx",
        ),
        (
            FactScope::Subagent("s".into()),
            "events_subagent_id_kind_idx",
        ),
        (FactScope::Workflow("w".into()), "events_run_id_kind_idx"),
        (
            FactScope::Interaction("i".into()),
            "events_interaction_id_kind_idx",
        ),
    ];
    for (scope, index) in scopes {
        for ascending in [false, true] {
            let query = FactQuery {
                scope: scope.clone(),
                kinds: vec!["attempt_started", "turn_started"],
                before: Some(99),
                after: 2,
                ascending,
                through: 100,
                limit: 32,
            };
            for (sql, params) in presentation_statements(&query).unwrap() {
                let plan = connection
                    .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
                    .unwrap()
                    .query_map(rusqlite::params_from_iter(params), |row| {
                        row.get::<_, String>(3)
                    })
                    .unwrap()
                    .collect::<Result<Vec<_>, _>>()
                    .unwrap()
                    .join("\n");
                assert!(
                    plan.contains(&format!("SEARCH events USING INDEX {index}")),
                    "{plan}"
                );
                assert!(
                    !plan.contains("SCAN ") && !plan.contains("TEMP B-TREE"),
                    "{plan}"
                );
            }
        }
    }
    for (name, expression) in PRESENTATION_INDEXES {
        connection
            .execute_batch(&format!("DROP INDEX {name}"))
            .unwrap();
        assert!(matches!(
            verify_schema_shape(&connection),
            Err(ConversationStoreError::IncompatibleSchema { .. })
        ));
        connection
            .execute_batch(&format!("CREATE INDEX {name} ON events(sequence)"))
            .unwrap();
        assert!(
            verify_schema_shape(&connection).is_err(),
            "a matching name is insufficient"
        );
        connection
            .execute_batch(&format!(
                "DROP INDEX {name}; CREATE INDEX {name} ON events({expression})"
            ))
            .unwrap();
        verify_schema_shape(&connection).unwrap();
    }
}

#[test]
fn schema_34_without_fixed_trace_index_contract_is_rejected() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("old.sqlite");
    let id = ConversationId::new("conv_a2b8feb8-c105-7980-8a72-d8482750c3fc");
    {
        let store = SqliteConversationStore::open(id.clone(), &path).unwrap();
        store
            .conn
            .lock()
            .unwrap()
            .execute(
                "UPDATE rustx_store SET schema_version = 34 WHERE id = 1",
                [],
            )
            .unwrap();
    }
    assert!(matches!(
        SqliteConversationStore::open(id, &path),
        Err(ConversationStoreError::SchemaVersionMismatch {
            stored: 34,
            expected: SQLITE_SCHEMA_VERSION
        })
    ));
}

#[test]
fn preceding_schema_is_rejected_before_event_deserialization() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("preceding.sqlite");
    let id = ConversationId::new("conv_497e1eed-f051-7f55-8d8c-4775b598e15c");
    {
        let store = SqliteConversationStore::open(id.clone(), &path).unwrap();
        let connection = store.conn.lock().unwrap();
        // Valid JSON but deliberately not a decodable RuntimeEventEnvelope.
        // The version gate must win before any payload reader can run.
        connection.execute(
            "INSERT INTO events(sequence,event_id,schema_version,conversation_id,attempt_id,turn_id,event_json,event_timestamp) VALUES(1,'incompatible',?1,?2,NULL,NULL,'{}','2026-01-01T00:00:00Z')",
            params![i64::from(EVENT_SCHEMA_VERSION), id.as_str()],
        ).unwrap();
        connection
            .execute(
                "UPDATE rustx_store SET schema_version=?1, next_event_sequence=1 WHERE id=1",
                params![SQLITE_SCHEMA_VERSION - 1],
            )
            .unwrap();
    }
    assert!(matches!(
        SqliteConversationStore::open(id, &path),
        Err(ConversationStoreError::SchemaVersionMismatch { stored, expected })
            if stored == SQLITE_SCHEMA_VERSION - 1 && expected == SQLITE_SCHEMA_VERSION
    ));
}

#[test]
fn workflow_node_wait_vocabulary_requires_current_store_format_without_rewrite() {
    use crate::runtime::identity::AuxiliaryCallId;
    use crate::runtime::workflow::read_model::WorkflowWait;

    assert_eq!(SQLITE_SCHEMA_VERSION, 81);
    assert_eq!(EVENT_SCHEMA_VERSION, 1);
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("workflow-wait.sqlite");
    let conversation = ConversationId::generate();
    let mut instance = crate::runtime::workflow::test_instance("model-format", "generate");
    instance.block.run.conversation_id = conversation.clone();
    let waiting = RuntimeEvent::WorkflowNodeWaiting {
        instance,
        reason: WorkflowWait::Model,
        auxiliary_call_id: Some(AuxiliaryCallId::generate()),
    };
    let committed = {
        let store = SqliteConversationStore::open(conversation.clone(), &path).unwrap();
        store
            .append_event(envelope(&conversation, "workflow-node-wait", None, waiting))
            .unwrap()
    };
    // The new vocabulary persists and decodes under the current database gate.
    {
        let current = SqliteConversationStore::open_existing(conversation.clone(), &path).unwrap();
        assert_eq!(current.read_events(None, 1).unwrap().events, [committed]);
    }
    {
        let connection = Connection::open(&path).unwrap();
        connection
            .execute("UPDATE rustx_store SET schema_version=77 WHERE id=1", [])
            .unwrap();
    }
    let before = std::fs::read(&path).unwrap();
    // Both constructors refuse the database before a Journal reader exists.
    for create_if_missing in [false, true] {
        let result = if create_if_missing {
            SqliteConversationStore::open(conversation.clone(), &path)
        } else {
            SqliteConversationStore::open_existing(conversation.clone(), &path)
        };
        assert!(matches!(
            result,
            Err(ConversationStoreError::SchemaVersionMismatch {
                stored: 77,
                expected: crate::durable::sqlite::SQLITE_SCHEMA_VERSION,
            })
        ));
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }
    let files = std::fs::read_dir(directory.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect::<Vec<_>>();
    assert_eq!(files, [std::ffi::OsString::from("workflow-wait.sqlite")]);
}

/// Pre-integration stores can hold retired workspace facts or lack the
/// combined canonical child-history contract. The gate refuses them at open,
/// before any row is decoded, and never rewrites, migrates or deletes it.
#[test]
fn pre_integration_stores_are_refused_and_left_untouched() {
    for previous_version in [50, 51, 55] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("pre-integration.sqlite");
        let id = ConversationId::new("conv_497e1eed-f051-7f55-8d8c-4775b598e15c");
        {
            let store = SqliteConversationStore::open(id.clone(), &path).unwrap();
            let connection = store.conn.lock().unwrap();
            for (sequence, event) in [
            r#"{"type":"workflow_workspace_owned","run_id":{"conversation_id":"conv_497e1eed-f051-7f55-8d8c-4775b598e15c","attempt_id":"attempt","invocation":1},"workspace":{"logical_workspace":"/w","isolation":{"type":"shared"}}}"#,
            r#"{"type":"subagent_workspace_disposal_started","subagent_id":"conv_497e1eed-f051-7f55-8d8c-4775b598e15c-subagent-1"}"#,
        ]
        .into_iter()
        .enumerate()
        {
            connection
                .execute(
                    "INSERT INTO events(sequence,event_id,schema_version,conversation_id,attempt_id,turn_id,event_json,event_timestamp) VALUES(?1,?2,?3,?4,NULL,NULL,?5,json_extract(?5,'$.timestamp'))",
                    params![
                        i64::try_from(sequence + 1).unwrap(),
                        format!("retired-{sequence}"),
                        i64::from(EVENT_SCHEMA_VERSION),
                        id.as_str(),
                        format!(
                            r#"{{"schema_version":{EVENT_SCHEMA_VERSION},"event_id":"retired-{sequence}","sequence":{},"conversation_id":"{}","timestamp":"2026-01-01T00:00:00Z","event":{event}}}"#,
                            sequence + 1,
                            id.as_str()
                        )
                    ],
                )
                .unwrap();
        }
            connection
            .execute(
                "UPDATE rustx_store SET schema_version = ?1, next_event_sequence = 3 WHERE id = 1",
                [previous_version],
            )
            .unwrap();
        }
        let before = std::fs::read(&path).unwrap();
        assert!(matches!(
            SqliteConversationStore::open(id, &path),
            Err(ConversationStoreError::SchemaVersionMismatch {
                stored,
                expected: SQLITE_SCHEMA_VERSION
            }) if stored == previous_version
        ));
        assert_eq!(
            std::fs::read(&path).unwrap(),
            before,
            "refusal never migrates, rewrites or deletes old data"
        );
        let mut sidecars = std::fs::read_dir(directory.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect::<Vec<_>>();
        sidecars.sort();
        assert_eq!(sidecars, ["pre-integration.sqlite"]);
    }
}

/// The other half of the gate: a store this runtime creates records the
/// version this runtime speaks, and reopens.
#[test]
fn a_current_store_records_the_typed_interaction_audit_schema_version() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("current.sqlite");
    let conversation_id = ConversationId::new("conv_497e1eed-f051-7f55-8d8c-4775b598e15c");
    let stored: i64 = {
        let store = SqliteConversationStore::open(conversation_id.clone(), &path).unwrap();
        let connection = store.conn.lock().unwrap();
        connection
            .query_row(
                "SELECT schema_version FROM rustx_store WHERE id=1",
                [],
                |row| row.get(0),
            )
            .unwrap()
    };
    assert_eq!(stored, SQLITE_SCHEMA_VERSION);
    SqliteConversationStore::open(conversation_id, &path).expect("a current store reopens");
}

#[test]
fn schema_29_without_loop_lifecycle_facts_is_rejected() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("pre-loop.sqlite");
    let conversation = ConversationId::new("conv_4406c108-b4ff-727c-8a70-ab32b1f0efd5");
    {
        let store = SqliteConversationStore::open(conversation.clone(), &path).unwrap();
        store
            .conn
            .lock()
            .unwrap()
            .execute(
                "UPDATE rustx_store SET schema_version = 29 WHERE id = 1",
                [],
            )
            .unwrap();
    }
    assert!(matches!(
        SqliteConversationStore::open(conversation, &path),
        Err(ConversationStoreError::SchemaVersionMismatch {
            stored: 29,
            expected: SQLITE_SCHEMA_VERSION
        })
    ));
}

#[test]
fn schema_27_without_workflow_recovery_guards_is_rejected() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("without-recovery-guard.sqlite");
    let conversation = ConversationId::new("conv_d85e9707-bae4-7aba-8e7e-3bf98a5f0a4b");
    {
        let store = SqliteConversationStore::open(conversation.clone(), &path).unwrap();
        store
            .conn
            .lock()
            .unwrap()
            .execute(
                "UPDATE rustx_store SET schema_version = 27 WHERE id = 1",
                [],
            )
            .unwrap();
    }
    assert!(matches!(
        SqliteConversationStore::open(conversation, &path),
        Err(ConversationStoreError::SchemaVersionMismatch {
            stored: 27,
            expected: SQLITE_SCHEMA_VERSION
        })
    ));
}

/// Issue #106 intentionally removes canonical System messages and adds
/// exact System-section/resource fields to `RequestSnapshot`. A version-2
/// development database is rejected at open; there is no migration or
/// compatibility decoder for the obsolete representation.
#[test]
fn pre_issue_106_schema_version_is_rejected_explicitly() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("pre-issue-106.sqlite");
    let conversation_id = ConversationId::new("conv_f9a4212a-8c18-76b4-8b37-69acee1c9a60");
    {
        let store = SqliteConversationStore::open(conversation_id.clone(), &path).unwrap();
        store
            .conn
            .lock()
            .unwrap()
            .execute("UPDATE rustx_store SET schema_version = 2 WHERE id = 1", [])
            .unwrap();
    }
    assert!(matches!(
        SqliteConversationStore::open(conversation_id, &path),
        Err(ConversationStoreError::SchemaVersionMismatch {
            stored: 2,
            expected: SQLITE_SCHEMA_VERSION
        })
    ));
}

/// Issue #111 adds the durable answer obligation
/// (`InboundTurnAdopted`) that startup recovery reads to decide whether an
/// adopted turn is still owed an answer.
///
/// A version-8 database predates that vocabulary: its adoption
/// transactions committed no obligation fact, so a current reader would
/// interpret "the fact is absent" as "no answer is owed" and silently
/// strand exactly the crash states the obligation rescues — a turn adopted
/// into canonical history and killed before its attempt reached a request
/// start. The physical tables are unchanged, which is precisely why the
/// version must gate open rather than the schema shape.
#[test]
fn pre_answer_obligation_schema_is_rejected_explicitly() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("pre-answer-obligation.sqlite");
    let conversation_id = ConversationId::new("conv_09e32769-fc6b-71ac-81e2-c993e7ff5861");
    {
        let store = SqliteConversationStore::open(conversation_id.clone(), &path).unwrap();
        store
            .conn
            .lock()
            .unwrap()
            .execute("UPDATE rustx_store SET schema_version = 8 WHERE id = 1", [])
            .unwrap();
    }
    assert!(matches!(
        SqliteConversationStore::open(conversation_id, &path),
        Err(ConversationStoreError::SchemaVersionMismatch {
            stored: 8,
            expected: SQLITE_SCHEMA_VERSION
        })
    ));
}

/// Issue #126 changes the durable interaction payload vocabulary from the
/// old Question/Answered contract to the structured Questionnaire
/// contract. A version-9 database therefore cannot be opened by the
/// current store, even though its physical tables are unchanged.
#[test]
fn pre_structured_questionnaire_schema_is_rejected_explicitly() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("pre-structured-questionnaire.sqlite");
    let conversation_id = ConversationId::new("conv_9be9531e-63d0-70aa-8359-c06d857d1479");
    {
        let store = SqliteConversationStore::open(conversation_id.clone(), &path).unwrap();
        store
            .conn
            .lock()
            .unwrap()
            .execute("UPDATE rustx_store SET schema_version = 9 WHERE id = 1", [])
            .unwrap();
    }
    assert!(matches!(
        SqliteConversationStore::open(conversation_id, &path),
        Err(ConversationStoreError::SchemaVersionMismatch {
            stored: 9,
            expected: SQLITE_SCHEMA_VERSION
        })
    ));
}

/// Issue #134 is one clean breaking development-schema transition from
/// the merged main schema (v11) to the retry-aware event format (v13).
/// There is no migration path for an earlier development schema.
#[test]
fn pre_issue_130_schema_version_is_rejected_explicitly() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("pre-issue-130.sqlite");
    let conversation_id = ConversationId::new("conv_e0814883-a446-7275-827d-b31701d9c6cc");
    {
        let store = SqliteConversationStore::open(conversation_id.clone(), &path).unwrap();
        store
            .conn
            .lock()
            .unwrap()
            .execute(
                "UPDATE rustx_store SET schema_version = 11 WHERE id = 1",
                [],
            )
            .unwrap();
    }
    assert!(matches!(
        SqliteConversationStore::open(conversation_id, &path),
        Err(ConversationStoreError::SchemaVersionMismatch {
            stored: 11,
            expected: SQLITE_SCHEMA_VERSION
        })
    ));
}

/// Issue #136 changes the durable cancellation status from an untyped
/// reason-only value to a reason-plus-phase value. The previous v13
/// development schema is rejected explicitly; no default phase is
/// supplied for old rows.
#[test]
fn issue136_schema_version_is_rejected_explicitly() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("pre-issue-136.sqlite");
    let conversation_id = ConversationId::new("conv_fa864790-c311-717f-8884-b4f0363af078");
    {
        let store = SqliteConversationStore::open(conversation_id.clone(), &path).unwrap();
        store
            .conn
            .lock()
            .unwrap()
            .execute(
                "UPDATE rustx_store SET schema_version = 13 WHERE id = 1",
                [],
            )
            .unwrap();
    }
    assert!(matches!(
        SqliteConversationStore::open(conversation_id, &path),
        Err(ConversationStoreError::SchemaVersionMismatch {
            stored: 13,
            expected: SQLITE_SCHEMA_VERSION
        })
    ));
}

/// Issue #137 adds the durable pending source pointer and frozen
/// request-only snapshot fields. The immediately previous development
/// schema is rejected explicitly; there is no legacy decoder or
/// migration for those fields.
#[test]
fn issue137_schema_version_is_rejected_explicitly() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("pre-issue-137.sqlite");
    let conversation_id = ConversationId::new("conv_a824ca2a-191a-74db-864b-400f12825324");
    {
        let store = SqliteConversationStore::open(conversation_id.clone(), &path).unwrap();
        store
            .conn
            .lock()
            .unwrap()
            .execute(
                "UPDATE rustx_store SET schema_version = 14 WHERE id = 1",
                [],
            )
            .unwrap();
    }
    assert!(matches!(
        SqliteConversationStore::open(conversation_id, &path),
        Err(ConversationStoreError::SchemaVersionMismatch {
            stored: 14,
            expected: SQLITE_SCHEMA_VERSION
        })
    ));
}

/// Issue #108 proposal-state rows are a development-only physical
/// contract. A version-5 database is rejected explicitly instead of
/// attempting to interpret its older proposal table or migrate it.
#[test]
fn pre_proposal_state_machine_schema_is_rejected_explicitly() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("pre-proposal-state-machine.sqlite");
    let conversation_id = ConversationId::new("conv_dfd0b924-9046-7f41-80a4-91c6b251f14c");
    {
        let store = SqliteConversationStore::open(conversation_id.clone(), &path).unwrap();
        store
            .conn
            .lock()
            .unwrap()
            .execute("UPDATE rustx_store SET schema_version = 5 WHERE id = 1", [])
            .unwrap();
    }
    assert!(matches!(
        SqliteConversationStore::open(conversation_id, &path),
        Err(ConversationStoreError::SchemaVersionMismatch {
            stored: 5,
            expected: SQLITE_SCHEMA_VERSION
        })
    ));
}
