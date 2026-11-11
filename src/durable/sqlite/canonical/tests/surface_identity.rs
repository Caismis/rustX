use crate::conversation::{SurfaceOp, SurfaceRevision};
use crate::durable::sqlite::SqliteConversationStore;
use crate::durable::sqlite::archive::Authority;
use crate::durable::sqlite::test_support::*;
use crate::durable::{CanonicalStore, ConversationStoreError, LineageSeed, ProjectionStore};
use crate::runtime::identity::{ConversationId, MessageId};

fn assert_invalid_reference<T: std::fmt::Debug>(result: &Result<T, ConversationStoreError>) {
    assert!(
        matches!(result, Err(ConversationStoreError::InvalidReference(_))),
        "expected InvalidReference, got {result:?}"
    );
}

fn assert_surface_readers_reject(store: &SqliteConversationStore) {
    let revision = SurfaceRevision::new(1);
    assert_invalid_reference(&store.reconstruct_surface(revision));
    assert_invalid_reference(&store.load_surface_history(revision));
    assert_invalid_reference(&store.load_surface_snapshot(revision));
    assert_invalid_reference(&store.load_user_message_boundaries(revision));
    assert_invalid_reference(&store.load_user_message_boundaries_page(revision, 0, 1));
    assert_invalid_reference(&store.read_lineage_cut(revision));
    assert_invalid_reference(&store.read_message_lineage_cut(
        revision,
        &MessageId::new("original"),
        true,
    ));
    let archived = store.archive_record(Authority::Surface, 0, 1);
    assert!(archived.is_err(), "damaged Surface exported: {archived:?}");
}

fn check_damage_before_and_after_reopen(damage: &str) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("surface.sqlite");
    let conversation = ConversationId::generate();
    let store = SqliteConversationStore::open(conversation.clone(), &path).unwrap();
    store
        .initialize(&[user_message("original", "body")])
        .unwrap();
    store
        .append_canonical(&user_message("later", "later body"))
        .unwrap();
    assert_eq!(
        store.reconstruct_surface(SurfaceRevision::new(1)).unwrap(),
        [MessageId::new("original")]
    );
    let raw = rusqlite::Connection::open(&path).unwrap();
    raw.execute_batch("PRAGMA foreign_keys=OFF").unwrap();
    raw.execute_batch(damage).unwrap();
    drop(raw);
    assert_surface_readers_reject(&store);
    drop(store);
    let reopened = SqliteConversationStore::open_existing(conversation, &path).unwrap();
    assert_surface_readers_reject(&reopened);
}

#[test]
fn historical_surface_rejects_relabelled_ledger_identity() {
    check_damage_before_and_after_reopen(
        "UPDATE message_ledger SET message_id='relabelled' WHERE position=1",
    );
}

#[test]
fn historical_surface_rejects_valid_but_wrong_ledger_position() {
    check_damage_before_and_after_reopen(
        "UPDATE surface_ops SET ledger_position=2 WHERE revision=1",
    );
}

#[test]
fn historical_surface_rejects_deleted_ledger_row() {
    check_damage_before_and_after_reopen("DELETE FROM message_ledger WHERE position=1");
}

#[test]
fn historical_surface_rejects_recreated_ledger_row() {
    check_damage_before_and_after_reopen(
        "DELETE FROM message_ledger WHERE position=1; INSERT INTO message_ledger(position,message_id,message_json) SELECT 1,'replacement',json_set(message_json,'$.id','replacement') FROM message_ledger WHERE position=2",
    );
}

#[test]
fn historical_surface_rejects_original_identity_recreated_elsewhere() {
    check_damage_before_and_after_reopen("UPDATE message_ledger SET position=3 WHERE position=1");
}

#[test]
fn historical_surface_boundaries_reject_valid_but_wrong_ledger_position() {
    let store = store();
    store
        .initialize(&[
            user_message("original", "body"),
            user_message("later", "body"),
        ])
        .unwrap();
    store
        .lock()
        .unwrap()
        .execute_batch(
            "PRAGMA foreign_keys=OFF; UPDATE surface_ops SET ledger_position=2 WHERE revision=1",
        )
        .unwrap();
    assert_invalid_reference(&store.load_user_message_boundaries_page(
        SurfaceRevision::new(1),
        0,
        1,
    ));
}

#[test]
fn historical_surface_archive_rejects_relabelled_ledger_identity() {
    let store = store();
    store
        .initialize(&[user_message("original", "body")])
        .unwrap();
    store.lock().unwrap().execute_batch("PRAGMA foreign_keys=OFF; UPDATE message_ledger SET message_id='relabelled' WHERE position=1").unwrap();
    let archived = store.archive_record(Authority::Surface, 0, 1);
    assert!(archived.is_err(), "damaged Surface exported: {archived:?}");
}

#[test]
fn historical_surface_lineage_copy_rejects_valid_but_wrong_ledger_position() {
    let store = store();
    store
        .initialize(&[
            user_message("original", "body"),
            user_message("later", "body"),
        ])
        .unwrap();
    store
        .lock()
        .unwrap()
        .execute_batch(
            "PRAGMA foreign_keys=OFF; UPDATE surface_ops SET ledger_position=2 WHERE revision=1",
        )
        .unwrap();
    assert_invalid_reference(&store.read_lineage_cut(SurfaceRevision::new(1)));
}

#[test]
fn historical_surface_lineage_copy_keeps_original_operations() {
    let source = store();
    source
        .initialize(&[user_message("original", "body")])
        .unwrap();
    source
        .append_canonical(&user_message("later", "body"))
        .unwrap();
    let cut = source.read_lineage_cut(SurfaceRevision::new(1)).unwrap();
    let seed = LineageSeed::replayed(cut.canonical, cut.surface_history).unwrap();
    let destination = store();
    destination.initialize_lineage(&seed).unwrap();
    destination
        .append_canonical(&user_message("destination-later", "body"))
        .unwrap();
    assert_eq!(
        destination
            .load_surface_history(SurfaceRevision::new(1))
            .unwrap(),
        [SurfaceOp::Append {
            message_id: MessageId::new("original")
        }]
    );
    destination.lock().unwrap().execute_batch("PRAGMA foreign_keys=OFF; UPDATE message_ledger SET message_id='relabelled' WHERE position=1").unwrap();
    assert_surface_readers_reject(&destination);
}
