//! Frozen request format boundaries across real file-backed Store opens.

use super::*;
use crate::model::structured::{ResponseSchema, StructuredOutputStrategy};
use crate::runtime::auxiliary::output::AuxiliaryOutputFormat;

fn persist_request(
    store: &SqliteConversationStore,
    structured: bool,
    native: bool,
) -> (RequestSnapshot, ModelRequest) {
    store
        .initialize(&[user_message("input", "frozen input")])
        .unwrap();
    let (mut snapshot, mut request) = super::manifest_store::selected_snapshot(store, 1);
    request.invocation.capabilities.json_schema_output = native;
    let format = if structured {
        AuxiliaryOutputFormat::JsonSchema(
            ResponseSchema::new(
                "stored_answer".into(),
                serde_json::json!({
                    "type": "object",
                    "properties": {"answer": {"type": "string"}},
                    "required": ["answer"],
                    "additionalProperties": false
                }),
                1024,
            )
            .unwrap(),
        )
    } else {
        AuxiliaryOutputFormat::Text
    };
    let request = format.prepare(request).unwrap();
    snapshot.invocation = request.invocation.clone();
    snapshot.effective_system_prompt = request.effective_system_prompt.clone();
    snapshot
        .freeze_input(store.conversation_id(), &request)
        .unwrap();
    store
        .commit_model_turn_start(&[], &snapshot, Utc::now())
        .unwrap();
    (snapshot, request)
}

#[test]
fn pre_structured_request_stores_are_rejected_at_open_without_rewrite() {
    for version in [75, 76, 77, 78, 79, 80] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("pre-structured.sqlite");
        let conversation = ConversationId::generate();
        {
            let store = SqliteConversationStore::open(conversation.clone(), &path).unwrap();
            persist_request(&store, false, false);
            // Fingerprint semantics require a version gate even when the
            // database contains only ordinary text requests.
            store
                .lock()
                .unwrap()
                .execute(
                    "UPDATE rustx_store SET schema_version=?1 WHERE id=1",
                    [version],
                )
                .unwrap();
        }
        let before = std::fs::read(&path).unwrap();
        for create_if_missing in [false, true] {
            let result = if create_if_missing {
                SqliteConversationStore::open(conversation.clone(), &path)
            } else {
                SqliteConversationStore::open_existing(conversation.clone(), &path)
            };
            assert!(matches!(result,
                Err(ConversationStoreError::SchemaVersionMismatch { stored, expected: crate::durable::sqlite::SQLITE_SCHEMA_VERSION })
                    if stored == version
            ));
            assert_eq!(std::fs::read(&path).unwrap(), before);
        }
    }
}

#[test]
fn current_format_text_and_structured_requests_reconstruct_exactly_after_reopen() {
    for structured in [false, true] {
        for native in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("current.sqlite");
            let conversation = ConversationId::generate();
            let (snapshot, expected) = {
                let store = SqliteConversationStore::open(conversation.clone(), &path).unwrap();
                let version: i64 = store
                    .lock()
                    .unwrap()
                    .query_row(
                        "SELECT schema_version FROM rustx_store WHERE id=1",
                        [],
                        |row| row.get(0),
                    )
                    .unwrap();
                assert_eq!(version, crate::durable::sqlite::SQLITE_SCHEMA_VERSION);
                persist_request(&store, structured, native)
            };
            for create_if_missing in [false, true] {
                let reopened = if create_if_missing {
                    SqliteConversationStore::open(conversation.clone(), &path)
                } else {
                    SqliteConversationStore::open_existing(conversation.clone(), &path)
                }
                .unwrap();
                assert_eq!(
                    reopened
                        .load_request_snapshot(&snapshot.request_id)
                        .unwrap(),
                    snapshot
                );
                let restored = reopened
                    .reconstruct_model_request(&snapshot.request_id)
                    .unwrap();
                assert_eq!(restored, expected);
                assert_eq!(
                    restored.semantic_input_fingerprint(),
                    snapshot.semantic_input_fingerprint
                );
                assert_eq!(restored.invocation.capabilities.json_schema_output, native);
                assert_eq!(
                    restored
                        .response_schema
                        .as_ref()
                        .map(|schema| schema.strategy),
                    structured.then_some(if native {
                        StructuredOutputStrategy::Native
                    } else {
                        StructuredOutputStrategy::Prompt
                    })
                );
            }
        }
    }
}
