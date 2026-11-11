use super::*;
use crate::model::projected::{DerivedModelMessage, ProjectedMessage, ProjectedUserKind};
use crate::model::{ModelInputMessage, SelectedInputManifest};

#[derive(serde::Serialize, schemars::JsonSchema)]
#[serde(transparent)]
struct FlatMessage<'a>(
    #[schemars(with = "crate::observation::trace::TraceRequestMessage")]
    super::super::input::Input<'a>,
);

fn append_message(index: usize) -> ModelInputMessage {
    ModelInputMessage::Derived(DerivedModelMessage {
        message: ProjectedMessage::User {
            content: vec![crate::message::types::UserContentBlock::Text(
                crate::message::content::TextBlock {
                    text: format!("constant append {index:04} 水🙂"),
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
        let message = append_message(history);
        if [8, 9, 64, 65, 512, 513, 4096, 4097, 8178, 8191].contains(&history) {
            snapshot.input = input.clone();
            let transaction = connection.transaction().unwrap();
            let selected = manifest::persist(&transaction, &input, &owner).unwrap();
            nodes::capture(
                &transaction,
                false,
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
            let captured = nodes::capture(
                &transaction,
                false,
                &scope,
                &next.messages(),
                &snapshot,
                &selected,
            )
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
        assert_old_cut(&connection, &scope, root, hash, &expected);
    }
}

fn assert_old_cut(
    connection: &Connection,
    scope: &crate::runtime::identity::ConversationId,
    root: manifest::NodeRef,
    hash: index::Hash,
    expected: &SelectedInputManifest,
) {
    assert_eq!(&manifest::load(connection, Some(root)).unwrap(), expected);
    for rank in [0, root.count / 2, root.count - 1] {
        let selected = nodes::select(connection, root, hash, 999, rank).unwrap();
        let mut bytes = fragments::bytes(
            connection,
            &selected.template.prefix,
            0,
            selected.template.prefix.length,
        )
        .unwrap();
        bytes.extend(nodes::hole(999, rank, selected.template.provenance_bytes).unwrap());
        bytes.extend(
            fragments::bytes(
                connection,
                &selected.template.suffix,
                0,
                selected.template.suffix.length,
            )
            .unwrap(),
        );
        let messages = expected.messages();
        let position = usize::try_from(rank).unwrap();
        let message = messages.get(position).unwrap();
        let frozen = expected.get(position).unwrap();
        let (mut view, _) = super::super::input_view(connection, scope, message, frozen).unwrap();
        let provenance_bytes = super::super::provenance::Raw::frozen(frozen, &view.provenance)
            .encoded_bytes()
            .unwrap();
        view.provenance_source = Some(super::super::provenance_source(999, rank, provenance_bytes));
        let oracle = serde_json::to_vec(
            &super::super::typed_value(&FlatMessage(super::super::input::Input {
                original: message,
                view: &view,
            }))
            .unwrap(),
        )
        .unwrap();
        assert_eq!(bytes, oracle, "old cut count={}, rank={rank}", root.count);
        assert_eq!(selected.length, oracle.len() as u64);
    }
}
