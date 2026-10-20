//! Canonical ledger, surface, and transcript regressions: initialization state.

use crate::durable::CanonicalStore;
use crate::durable::sqlite::test_support::*;
use crate::durable::sqlite::*;
use crate::durable::{ConversationStoreError, LineageSeed};
use crate::runtime::identity::ConversationId;

pub(super) fn deny_bootstrap_payloads(
    context: rusqlite::hooks::AuthContext<'_>,
) -> rusqlite::hooks::Authorization {
    use rusqlite::hooks::{AuthAction, Authorization};
    match context.action {
        AuthAction::Read {
            table_name,
            column_name,
            ..
        } => match (table_name, column_name) {
            // SQLite also authorizes row-presence access with an empty column
            // name for this rowid-only LEFT JOIN. It extracts no payload.
            ("rustx_store", "id" | "bootstrap_initialized") | ("bootstrap_identity", "id" | "") => {
                Authorization::Allow
            }
            _ => Authorization::Deny,
        },
        _ => Authorization::Allow,
    }
}
#[test]
fn native_initialization_state_distinguishes_fresh_empty_and_inherited_without_payload_reads() {
    let store = store();
    store
        .lock()
        .unwrap()
        .authorizer(Some(deny_bootstrap_payloads))
        .unwrap();
    for sql in [
        "SELECT occupancy_json FROM bootstrap_identity WHERE id=1",
        "SELECT message_json FROM message_ledger WHERE position=1",
    ] {
        let error = store.lock().unwrap().prepare(sql).err().unwrap();
        assert_eq!(
            error.sqlite_error_code(),
            Some(rusqlite::ErrorCode::AuthorizationForStatementDenied),
            "{sql}"
        );
    }
    assert!(!store.is_initialized().unwrap());
    store
        .lock()
        .unwrap()
        .authorizer(None::<fn(rusqlite::hooks::AuthContext<'_>) -> rusqlite::hooks::Authorization>)
        .unwrap();
    store.initialize(&[]).unwrap();
    store
        .lock()
        .unwrap()
        .authorizer(Some(deny_bootstrap_payloads))
        .unwrap();
    assert!(
        store.is_initialized().unwrap(),
        "initialized empty is not fresh"
    );
    store
        .lock()
        .unwrap()
        .authorizer(None::<fn(rusqlite::hooks::AuthContext<'_>) -> rusqlite::hooks::Authorization>)
        .unwrap();
    let inherited = crate::durable::sqlite::test_support::store();
    inherited
        .initialize_lineage(&LineageSeed::history(vec![user_message(
            "inherited-init",
            &"水".repeat(100_000),
        )]))
        .unwrap();
    inherited
        .lock()
        .unwrap()
        .authorizer(Some(deny_bootstrap_payloads))
        .unwrap();
    assert!(inherited.is_initialized().unwrap());
}
#[test]
fn native_initialization_state_reopens_the_same_committed_bootstrap_identity() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("bootstrap.sqlite");
    let conversation = ConversationId::new("conv_012a15b6-1fab-7b45-a849-f22b681fd8ab");
    let messages = vec![user_message(
        "native-bootstrap-original",
        "inherited original",
    )];
    {
        let store = SqliteConversationStore::open(conversation.clone(), &path).unwrap();
        assert!(!store.is_initialized().unwrap());
        store
            .initialize_lineage(&LineageSeed::history(messages.clone()))
            .unwrap();
        assert!(store.is_initialized().unwrap());
    }
    let reopened = SqliteConversationStore::open(conversation, &path).unwrap();
    reopened
        .lock()
        .unwrap()
        .authorizer(Some(deny_bootstrap_payloads))
        .unwrap();
    assert!(reopened.is_initialized().unwrap());
    reopened
        .lock()
        .unwrap()
        .authorizer(None::<fn(rusqlite::hooks::AuthContext<'_>) -> rusqlite::hooks::Authorization>)
        .unwrap();
    assert_eq!(reopened.load_bootstrap_history().unwrap(), messages);
}
#[test]
fn native_initialization_state_refuses_mismatched_or_malformed_marker_identity_pairs() {
    for (initialized, mutation) in [
        (true, "DELETE FROM bootstrap_identity WHERE id=1"),
        (
            true,
            "UPDATE rustx_store SET bootstrap_initialized=0 WHERE id=1",
        ),
        (
            false,
            "UPDATE rustx_store SET bootstrap_initialized=1 WHERE id=1",
        ),
        (
            true,
            "UPDATE rustx_store SET bootstrap_initialized=2 WHERE id=1",
        ),
        (
            true,
            "UPDATE rustx_store SET bootstrap_initialized=X'01' WHERE id=1",
        ),
    ] {
        let store = store();
        if initialized {
            store.initialize(&[]).unwrap();
        }
        store
            .lock()
            .unwrap()
            .execute_batch("PRAGMA ignore_check_constraints=ON")
            .unwrap();
        store.lock().unwrap().execute_batch(mutation).unwrap();
        store
            .lock()
            .unwrap()
            .execute_batch("PRAGMA ignore_check_constraints=OFF")
            .unwrap();
        assert!(
            matches!(
                store.is_initialized(),
                Err(ConversationStoreError::Storage(_))
            ),
            "{mutation}"
        );
    }
}
