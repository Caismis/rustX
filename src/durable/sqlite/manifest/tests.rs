use super::{
    Connection, ConversationStoreError, Ordering, Owner, create_schema, load, load_bounded,
    persist, verify_schema,
};
use crate::model::SelectedInputManifest;
use crate::runtime::identity::{ConversationId, MessageId};
use chrono::Utc;

fn message(index: usize) -> crate::model::ModelInputMessage {
    crate::message::types::MessageBlock::User(crate::message::types::UserMessageBlock {
        id: MessageId::new(format!("manifest-{index}")),
        source: crate::message::types::UserSource::Human,
        kind: crate::message::types::InboundKind::Message,
        content: vec![crate::message::types::UserContentBlock::Text(
            crate::message::content::TextBlock {
                text: format!("body-{index}"),
            },
        )],
        timestamp: Some(Utc::now()),
    })
    .into()
}
fn connection() -> Connection {
    let connection = Connection::open_in_memory().unwrap();
    connection.execute_batch("PRAGMA foreign_keys=ON; CREATE TABLE message_ledger(position INTEGER PRIMARY KEY,message_id TEXT,message_json TEXT,observation_header TEXT); CREATE TABLE projection_head(id INTEGER PRIMARY KEY,activation INTEGER); CREATE TABLE projection_checkpoints(checkpoint_id TEXT);").unwrap();
    create_schema(&connection).unwrap();
    connection
}

fn cached_shape_fixture(capacity: usize) -> (Connection, super::NodeRef, super::NodeRef) {
    let mut connection = connection();
    let owner = Owner::install(
        &connection,
        super::super::append_validation::Owner::uninstalled(),
    )
    .unwrap();
    let scope = ConversationId::generate();
    let mut roots = Vec::new();
    for messages in [vec![message(1)], vec![message(1), message(2)]] {
        let input = SelectedInputManifest::from_messages(&scope, &messages);
        let transaction = connection.transaction().unwrap();
        let selection = persist(&transaction, &input, &owner).unwrap();
        roots.push(selection.root.unwrap());
        transaction.commit().unwrap();
        selection.commit(&owner);
    }
    connection.set_prepared_statement_cache_capacity(capacity);
    (connection, roots[0], roots[1])
}

#[test]
fn native_shape_statement_reuses_preparation_and_rebinds_leaf_and_branch_ids() {
    use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
    use std::sync::{Arc, atomic::AtomicUsize};
    for capacity in [0, 16] {
        let (connection, leaf, branch) = cached_shape_fixture(capacity);
        let prepares = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&prepares);
        connection
            .authorizer(Some(move |context: AuthContext<'_>| {
                if matches!(
                    context.action,
                    AuthAction::Read {
                        table_name: "native_input_nodes",
                        column_name: "item_digest",
                        ..
                    }
                ) {
                    observed.fetch_add(1, Ordering::SeqCst);
                }
                Authorization::Allow
            }))
            .unwrap();
        assert!(matches!(
            super::node(&connection, &leaf).unwrap().kind,
            super::NodeKind::Leaf
        ));
        assert!(prepares.swap(0, Ordering::SeqCst) > 0);
        assert!(matches!(
            super::node(&connection, &branch).unwrap().kind,
            super::NodeKind::Branch { .. }
        ));
        let repeated = prepares.swap(0, Ordering::SeqCst);
        if capacity == 0 {
            assert!(repeated > 0);
        } else {
            assert_eq!(
                repeated, 0,
                "shape lookup must reuse SQL, not its prior row"
            );
        }
        assert!(matches!(
            super::node(&connection, &leaf).unwrap().kind,
            super::NodeKind::Leaf
        ));
    }
}

#[test]
fn warmed_native_shape_reads_reject_changed_leaf_and_branch_evidence() {
    for capacity in [0, 16] {
        let (connection, leaf, branch) = cached_shape_fixture(capacity);
        for (node, column) in [(leaf, "item_digest"), (branch, "left_digest")] {
            super::node(&connection, &node).unwrap();
            let original: Vec<u8> = connection
                .query_row(
                    &format!("SELECT {column} FROM native_input_nodes WHERE id=?1"),
                    [node.id],
                    |row| row.get(0),
                )
                .unwrap();
            connection
                .execute(
                    &format!("UPDATE native_input_nodes SET {column}=zeroblob(32) WHERE id=?1"),
                    [node.id],
                )
                .unwrap();
            assert!(super::node(&connection, &node).is_err());
            connection
                .execute(
                    &format!("UPDATE native_input_nodes SET {column}=?2 WHERE id=?1"),
                    rusqlite::params![node.id, original],
                )
                .unwrap();
            super::node(&connection, &node).unwrap();
        }
    }
}

#[test]
fn warmed_native_shape_statement_obeys_later_read_authorization() {
    use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
    for capacity in [0, 16] {
        let (connection, leaf, branch) = cached_shape_fixture(capacity);
        super::node(&connection, &leaf).unwrap();
        super::node(&connection, &branch).unwrap();
        connection
            .authorizer(Some(|context: AuthContext<'_>| {
                if matches!(
                    context.action,
                    AuthAction::Read {
                        table_name: "native_input_nodes",
                        column_name: "item_digest",
                        ..
                    }
                ) {
                    Authorization::Deny
                } else {
                    Authorization::Allow
                }
            }))
            .unwrap();
        assert!(super::node(&connection, &branch).is_err());
        connection
            .authorizer(None::<fn(AuthContext<'_>) -> Authorization>)
            .unwrap();
        super::node(&connection, &branch).unwrap();
    }
}
#[test]
fn native_manifest_rollback_never_certifies_absent_or_reused_row_ids() {
    let mut connection = connection();
    let owner = Owner::install(
        &connection,
        super::super::append_validation::Owner::uninstalled(),
    )
    .unwrap();
    let scope = ConversationId::generate();
    let selected = SelectedInputManifest::from_messages(&scope, &[message(1)]);
    let discarded = {
        let transaction = connection.transaction().unwrap();
        let selection = persist(&transaction, &selected, &owner).unwrap();
        assert_eq!(selection.seats.len(), 1);
        transaction.rollback().unwrap();
        selection
    };
    assert!(
        selected
            .root
            .as_ref()
            .unwrap()
            .stored
            .lock()
            .unwrap()
            .is_none()
    );
    drop(discarded);
    let transaction = connection.transaction().unwrap();
    let fresh = persist(&transaction, &selected, &owner).unwrap();
    assert_eq!(fresh.seats.len(), 1);
    let root = fresh.root;
    transaction.commit().unwrap();
    fresh.commit(&owner);
    assert_eq!(load(&connection, root).unwrap(), selected);
    let warm = persist(&connection, &selected, &owner).unwrap();
    assert!(warm.seats.is_empty());
    assert_eq!(warm.root, root);
}
#[test]
fn native_manifest_append_persists_only_delta_paths_and_retains_old_roots() {
    let mut connection = connection();
    let owner = Owner::install(
        &connection,
        super::super::append_validation::Owner::uninstalled(),
    )
    .unwrap();
    let scope = ConversationId::generate();
    let mut selected = SelectedInputManifest::default();
    let mut retained = Vec::new();
    for index in 0..300 {
        selected = selected.append(&scope, &message(index));
        let transaction = connection.transaction().unwrap();
        let selection = persist(&transaction, &selected, &owner).unwrap();
        let root = selection.root.unwrap();
        assert!(selection.seats.len() <= usize::from(root.height) + 2);
        assert_eq!(
            selection
                .seats
                .iter()
                .filter(|(_, n)| n.count == 1 && n.height == 1)
                .count(),
            1
        );
        transaction.commit().unwrap();
        selection.commit(&owner);
        if index == 99 || index == 299 {
            retained.push((selected.clone(), root));
        }
    }
    for (original, root) in retained {
        assert_eq!(load(&connection, Some(root)).unwrap(), original);
    }
}
#[test]
fn native_mutation_certificate_covers_trigger_replace_delete_and_external_commit() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("manifest.sqlite");
    let connection = Connection::open(&path).unwrap();
    connection.execute_batch("CREATE TABLE message_ledger(position INTEGER PRIMARY KEY,message_id TEXT,message_json TEXT,observation_header TEXT); CREATE TABLE projection_head(id INTEGER PRIMARY KEY,activation INTEGER); CREATE TABLE projection_checkpoints(checkpoint_id TEXT); INSERT INTO message_ledger VALUES(1,'one','original',NULL); CREATE TABLE start_probe(id INTEGER); CREATE TRIGGER mutate_original AFTER INSERT ON start_probe BEGIN UPDATE message_ledger SET message_json='changed' WHERE position=1; END;").unwrap();
    create_schema(&connection).unwrap();
    let owner = Owner::install(
        &connection,
        super::super::append_validation::Owner::uninstalled(),
    )
    .unwrap();
    owner.frontier.store(1, Ordering::Release);
    let before = owner.capture(&connection).unwrap();
    connection
        .execute(
            "UPDATE message_ledger SET observation_header='display only'",
            [],
        )
        .unwrap();
    assert_eq!(owner.capture(&connection).unwrap(), before);
    connection
        .execute("INSERT INTO start_probe VALUES(1)", [])
        .unwrap();
    let triggered = owner.capture(&connection).unwrap();
    assert!(triggered > before);
    connection.execute("INSERT OR REPLACE INTO message_ledger(position,message_id,message_json) VALUES(1,'one','replacement')",[]).unwrap();
    let replaced = owner.capture(&connection).unwrap();
    assert!(replaced > triggered);
    connection
        .execute("DELETE FROM message_ledger", [])
        .unwrap();
    let deleted = owner.capture(&connection).unwrap();
    assert!(deleted > replaced);
    let other = Connection::open(path).unwrap();
    other
        .execute(
            "INSERT INTO message_ledger VALUES(2,'two','external',NULL)",
            [],
        )
        .unwrap();
    assert!(owner.capture(&connection).unwrap() > deleted);
}
#[test]
fn native_selected_leaf_corruption_and_logical_byte_limit_are_explicit() {
    let mut connection = connection();
    let owner = Owner::install(
        &connection,
        super::super::append_validation::Owner::uninstalled(),
    )
    .unwrap();
    let selected = SelectedInputManifest::from_messages(&ConversationId::generate(), &[message(1)]);
    let transaction = connection.transaction().unwrap();
    let selection = persist(&transaction, &selected, &owner).unwrap();
    let root = selection.root.unwrap();
    transaction.commit().unwrap();
    selection.commit(&owner);
    let exact = serde_json::to_vec(&selected).unwrap();
    assert_eq!(
        load_bounded(&connection, Some(root), exact.len()).unwrap(),
        selected
    );
    assert!(load_bounded(&connection, Some(root), exact.len() - 1).is_err());
    connection
        .execute(
            "UPDATE native_input_nodes SET item_json='null' WHERE id=?1",
            [root.id],
        )
        .unwrap();
    assert!(matches!(
        load(&connection, Some(root)),
        Err(ConversationStoreError::InvalidReference(_))
    ));
}

#[test]
fn native_manifest_schema_missing_or_replaced_table_is_rejected_before_owner_install() {
    let connection = connection();
    verify_schema(&connection).unwrap();
    connection
        .execute_batch("DROP TABLE native_input_nodes")
        .unwrap();
    assert!(matches!(
        verify_schema(&connection),
        Err(ConversationStoreError::IncompatibleSchema(_))
    ));
    connection.execute_batch("CREATE TABLE native_input_nodes(id INTEGER PRIMARY KEY,digest BLOB,count INTEGER,height INTEGER,item_json TEXT)").unwrap();
    assert!(matches!(
        verify_schema(&connection),
        Err(ConversationStoreError::IncompatibleSchema(_))
    ));
}

#[test]
fn native_journal_certificate_distinguishes_new_append_from_old_fact_or_auxiliary_update() {
    let connection = connection();
    connection.execute_batch("CREATE TABLE events(sequence INTEGER PRIMARY KEY,event_id TEXT,event_json TEXT,observation_witness BLOB); INSERT INTO events VALUES(1,'original','original',NULL);").unwrap();
    let owner = Owner::install(
        &connection,
        super::super::append_validation::Owner::uninstalled(),
    )
    .unwrap();
    owner.journal_frontier.store(1, Ordering::Release);
    let before = owner.capture(&connection).unwrap();
    connection
        .execute("INSERT INTO events VALUES(2,'new','new',NULL)", [])
        .unwrap();
    connection
        .execute(
            "UPDATE events SET observation_witness=x'01' WHERE sequence=1",
            [],
        )
        .unwrap();
    assert_eq!(owner.capture(&connection).unwrap(), before);
    connection
        .execute(
            "UPDATE events SET event_json='changed' WHERE sequence=1",
            [],
        )
        .unwrap();
    let changed = owner.capture(&connection).unwrap();
    assert!(changed > before);
    connection
        .execute("DELETE FROM events WHERE sequence=1", [])
        .unwrap();
    let removed = owner.capture(&connection).unwrap();
    assert!(removed > changed);
    connection
        .execute("INSERT INTO events VALUES(1,'reused','reused',NULL)", [])
        .unwrap();
    assert!(owner.capture(&connection).unwrap() > removed);
}

#[test]
fn actual_only_model_input_is_rejected_before_native_sql_persistence() {
    let connection = connection();
    let owner = Owner::install(
        &connection,
        super::super::append_validation::Owner::uninstalled(),
    )
    .unwrap();
    let input: crate::model::SelectedModelInput = vec![message(1)].into();
    super::take_node_read_work();
    assert!(matches!(
        persist(&connection, &input.0, &owner),
        Err(ConversationStoreError::InvalidReference(_))
    ));
    assert_eq!(super::take_node_read_work(), (0, 0));
    assert_eq!(
        connection
            .query_row("SELECT count(*) FROM native_input_nodes", [], |row| row
                .get::<_, u64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn wal_external_invalidation_tracks_the_pinned_snapshot() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("snapshot.sqlite");
    drop(crate::durable::SqliteConversationStore::open(ConversationId::generate(), &path).unwrap());
    let mut connection = Connection::open(&path).unwrap();
    connection.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE probe(value INTEGER); INSERT INTO probe VALUES(1)").unwrap();
    let owner = Owner::install(
        &connection,
        super::super::append_validation::Owner::uninstalled(),
    )
    .unwrap();
    let before = owner.capture(&connection).unwrap();
    let transaction = connection.transaction().unwrap();
    assert_eq!(owner.capture(&transaction).unwrap(), before);
    let writer = Connection::open(&path).unwrap();
    writer.execute("UPDATE probe SET value=2", []).unwrap();
    assert_eq!(
        transaction
            .query_row("SELECT value FROM probe", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(owner.capture(&transaction).unwrap(), before);
    transaction.rollback().unwrap();
    assert!(owner.capture(&connection).unwrap() > before);
    assert_eq!(
        connection
            .query_row("SELECT value FROM probe", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        2
    );
}
