use super::*;
use crate::model::projected::{DerivedModelMessage, ProjectedMessage, ProjectedUserKind};
use crate::model::{ModelInputMessage, SelectedInputManifest};

fn append_message() -> ModelInputMessage {
    ModelInputMessage::Derived(DerivedModelMessage {
        message: ProjectedMessage::User {
            content: vec![crate::message::types::UserContentBlock::Text(
                crate::message::content::TextBlock {
                    text: "constant append 水🙂".into(),
                },
            )],
            source: crate::message::types::UserSource::Human,
            kind: ProjectedUserKind::Message,
            timestamp: None,
        },
        sources: Vec::new(),
        checkpoint_sources: Vec::new(),
        transformations: Vec::new(),
    })
}

#[test]
fn append_source_cost_is_bounded_at_every_history_size() {
    let mut connection = Connection::open_in_memory().unwrap();
    manifest::create_schema(&connection).unwrap();
    schema::create_schema(&connection).unwrap();
    install_workspace(&connection).unwrap();
    let owner = manifest::Owner::uninstalled();
    let scope = crate::runtime::identity::ConversationId::generate();
    let message = append_message();
    let mut input = SelectedInputManifest::default();
    let mut snapshot = crate::durable::sqlite::test_support::progress_request_snapshot(
        "s3",
        "1",
        0,
        crate::conversation::SurfaceRevision::default(),
        Vec::new(),
    );
    let mut old_roots = Vec::new();
    for history in 0..=8191 {
        if [8, 9, 64, 65, 512, 513, 4096, 4097, 8178, 8191].contains(&history) {
            snapshot.input = input.clone();
            let transaction = connection.transaction().unwrap();
            let selected = manifest::persist(&transaction, &input, &owner).unwrap();
            nodes::capture(
                &transaction,
                &scope,
                &input.messages(),
                &snapshot,
                &selected,
            )
            .unwrap();
            transaction.commit().unwrap();
            selected.commit(&owner);
            let payload = |c: &Connection| -> u64 {
                c.query_row("SELECT coalesce(sum(payload),0) FROM dbstat WHERE name IN ('native_input_nodes','native_input_items','request_source_pages')", [], |r| r.get(0)).unwrap()
            };
            let before = payload(&connection);
            let pages_before: u64 = connection
                .query_row("SELECT count(*) FROM request_source_pages", [], |row| {
                    row.get(0)
                })
                .unwrap();
            let next = input.append(&scope, &message);
            snapshot.input = next.clone();
            let transaction = connection.transaction().unwrap();
            take_work();
            let selected = manifest::persist(&transaction, &next, &owner).unwrap();
            let captured =
                nodes::capture(&transaction, &scope, &next.messages(), &snapshot, &selected)
                    .unwrap()
                    .unwrap();
            let work = take_work();
            assert_eq!(work.new_leaves, 1);
            assert!(work.new_nodes <= 4, "H={history}: {work:?}");
            let root = selected.root.unwrap();
            let hash = captured.digest;
            transaction.commit().unwrap();
            selected.commit(&owner);
            let bytes = payload(&connection) - before;
            let pages_after: u64 = connection
                .query_row("SELECT count(*) FROM request_source_pages", [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert!(
                pages_after - pages_before <= 16,
                "fixed small appended body: H={history}"
            );
            assert!(
                bytes <= 8192,
                "fixed small appended body: H={history}, bytes={bytes}"
            );
            eprintln!(
                "SOURCE_APPEND_COST H={history} nodes={} page_rows={} payload_bytes={bytes}",
                work.new_nodes,
                pages_after - pages_before
            );
            old_roots.push((root, hash, next));
        }
        input = input.append(&scope, &message);
    }
    for (root, hash, expected) in old_roots {
        assert_eq!(manifest::load(&connection, Some(root)).unwrap(), expected);
        for rank in [0, root.count / 2, root.count - 1] {
            assert!(
                nodes::select(&connection, root, hash, 999, rank)
                    .unwrap()
                    .length
                    > 0
            );
        }
    }
}
