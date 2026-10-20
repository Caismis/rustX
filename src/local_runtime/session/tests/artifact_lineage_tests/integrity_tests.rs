//! Actual catalog preparations must preserve the source artifact's frozen bytes.
use super::super::super::LineageSide;
use super::*;
use crate::durable::{CanonicalStore, JournalStore, ObservationStore};
use crate::message::types::ContentBlockIndex;
use crate::observation::content::{
    CanonicalContentComponent, CanonicalContentLocator, ContentRef, ContentRepresentation,
    ContentSource,
};
use base64::Engine as _;

const ORIGINAL: &[u8] = b"immutable artifact evidence";

#[derive(Clone, Copy, Debug)]
enum CopyKind {
    Clone,
    Fork,
    Tree,
}

const KINDS: [CopyKind; 3] = [CopyKind::Clone, CopyKind::Fork, CopyKind::Tree];

impl CopyKind {
    fn prepare(
        self,
        catalog: &SessionCatalog,
        cut: &LineageReadCut,
    ) -> Result<PreparedLineage, SessionError> {
        match self {
            Self::Clone => catalog.prepare_clone_session(&state(), cut),
            Self::Fork => catalog
                .prepare_fork_session(
                    &state(),
                    cut,
                    &MessageId::new("prompt-2"),
                    LineageSide::Before,
                )
                .map(|(prepared, _)| prepared),
            Self::Tree => catalog
                .prepare_tree_node(
                    &first_session(catalog),
                    &state(),
                    cut,
                    &MessageId::new("prompt-2"),
                    LineageSide::Before,
                )
                .map(|(prepared, _)| prepared),
        }
    }

    fn publish(self, catalog: &mut SessionCatalog, prepared: &PreparedLineage) {
        if matches!(self, Self::Tree) {
            let (node, _) = catalog.lineage(&prepared.session_id, None).unwrap();
            catalog
                .publish_node(
                    &prepared.session_id,
                    prepared,
                    node.id,
                    SessionNodeOrigin::New,
                )
                .unwrap();
        } else {
            catalog
                .publish_session(prepared, SessionNodeOrigin::New)
                .unwrap();
        }
    }
}

fn binary_source(store: &SqliteConversationStore) -> ContentSource {
    let first = store
        .load_canonical()
        .unwrap()
        .into_iter()
        .next()
        .expect("canonical prompt");
    ContentSource::CanonicalMessageBlock {
        component: CanonicalContentComponent::Payload,
        locator: CanonicalContentLocator::ByMessageId {
            message_id: first.id().clone(),
        },
        block_index: ContentBlockIndex::new(1),
    }
}

fn binary_reference(store: &SqliteConversationStore) -> ContentRef {
    store
        .content_descriptor(&binary_source(store), ContentRepresentation::BinaryBytes)
        .unwrap()
        .expect("original canonical binary reference")
}

fn source_without_optional_binary_index(
    catalog: &SessionCatalog,
) -> (SqliteConversationStore, LineageReadCut) {
    let (conversation, session, _) = append_history(catalog, &[]);
    let database = catalog.database_path(&session, &conversation);
    let connection = rusqlite::Connection::open(database).unwrap();
    connection.execute_batch("CREATE TRIGGER refuse_optional_binary_index BEFORE INSERT ON canonical_binary_segments BEGIN SELECT RAISE(FAIL,'optional binary index fault'); END").unwrap();
    let (store, cut) = source(catalog, true);
    for table in ["canonical_binary_sources", "canonical_binary_segments"] {
        let count: u64 = connection
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 0, "failed optional index rolls back {table}");
    }
    let frontiers: u64 = connection
        .query_row(
            "SELECT count(*) FROM content_frontiers WHERE representation='binary_bytes'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(frontiers, 0);
    assert!(
        store
            .content_descriptor(&binary_source(&store), ContentRepresentation::BinaryBytes)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        cut.turns.len(),
        2,
        "native history commits despite optional failure"
    );
    (store, cut)
}

fn assert_original(store: &SqliteConversationStore) {
    let reference = binary_reference(store);
    let chunk = store.content_read(&reference, 0, 65_536).unwrap();
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(chunk.data_base64)
            .unwrap(),
        ORIGINAL
    );
    assert_eq!(chunk.end_offset, u64::try_from(ORIGINAL.len()).unwrap());
    assert!(chunk.complete);
}

fn allocation_paths(root: &std::path::Path) -> BTreeSet<std::path::PathBuf> {
    fn visit(
        root: &std::path::Path,
        directory: &std::path::Path,
        paths: &mut BTreeSet<std::path::PathBuf>,
    ) {
        for entry in fs::read_dir(directory).unwrap() {
            let entry = entry.unwrap();
            paths.insert(entry.path().strip_prefix(root).unwrap().to_path_buf());
            if entry.file_type().unwrap().is_dir() {
                visit(root, &entry.path(), paths);
            }
        }
    }
    let mut paths = BTreeSet::new();
    visit(root, root, &mut paths);
    paths
}

#[test]
fn clone_fork_tree_reject_equal_length_artifact_tamper_without_catalog_or_allocation_leaks() {
    for kind in KINDS {
        let (_root, catalog, _) = open_catalog();
        let (store, cut) = source(&catalog, true);
        assert_original(&store);
        let reference = binary_reference(&store);
        let source_path = catalog.database_path(&first_session(&catalog), &cut.conversation_id);
        fs::write(
            source_path.parent().unwrap().join("artifact_2.bin"),
            vec![b'B'; ORIGINAL.len()],
        )
        .unwrap();
        assert!(store.content_read(&reference, 0, 65_536).is_err());
        let catalog_before = fs::read(&catalog.path).unwrap();
        let allocations_before = allocation_paths(&catalog.root);
        let canonical_before = store.load_canonical().unwrap();

        let error = kind
            .prepare(&catalog, &cut)
            .expect_err("changed bytes cannot become a new Original");
        assert!(
            !error.committed(),
            "{kind:?}: failed copy cannot publish catalog state"
        );
        assert_eq!(fs::read(&catalog.path).unwrap(), catalog_before, "{kind:?}");
        assert_eq!(
            allocation_paths(&catalog.root),
            allocations_before,
            "{kind:?}: private destination removed"
        );
        assert_eq!(store.load_canonical().unwrap(), canonical_before);
        assert_eq!(catalog.list_page(None, 0, 32).unwrap().sessions.len(), 1);
    }
}

#[test]
fn clone_fork_tree_own_original_binary_bytes_after_source_artifact_removal_and_cold_reopen() {
    for kind in KINDS {
        let (root, mut catalog, _) = open_catalog();
        let (store, cut) = source(&catalog, true);
        let source_path = catalog.database_path(&first_session(&catalog), &cut.conversation_id);
        let prepared = kind.prepare(&catalog, &cut).unwrap();
        kind.publish(&mut catalog, &prepared);
        drop(store);
        fs::remove_file(source_path.parent().unwrap().join("artifact_2.bin")).unwrap();
        drop(catalog);

        let reopened_catalog = reopen_catalog(root.path());
        let copied = store_for(
            &reopened_catalog,
            &prepared.session_id,
            &prepared.conversation_id,
        );
        assert_original(&copied);
        let reference = binary_reference(&copied);
        assert_eq!(reference.conversation_id, prepared.conversation_id);
        assert_ne!(reference.conversation_id, cut.conversation_id);
        assert!(reopened_catalog.snapshot(&prepared.session_id).is_ok());
        assert_eq!(copied.presentation_frontier().unwrap(), 0);
    }
}

#[test]
fn clone_fork_tree_copy_healthy_artifacts_despite_optional_binary_index_rollback() {
    for kind in KINDS {
        let (_root, mut catalog, _) = open_catalog();
        let (store, cut) = source_without_optional_binary_index(&catalog);
        let prepared = kind.prepare(&catalog, &cut).unwrap();
        kind.publish(&mut catalog, &prepared);
        let copied = store_for(&catalog, &prepared.session_id, &prepared.conversation_id);
        assert_original(&copied);
        assert_eq!(store.load_canonical().unwrap(), cut.canonical);
    }
}

#[test]
fn clone_fork_tree_reject_tamper_even_when_optional_binary_index_rolled_back() {
    for kind in KINDS {
        let (_root, catalog, _) = open_catalog();
        let (store, cut) = source_without_optional_binary_index(&catalog);
        let source_path = catalog.database_path(&first_session(&catalog), &cut.conversation_id);
        fs::write(
            source_path.parent().unwrap().join("artifact_2.bin"),
            vec![b'B'; ORIGINAL.len()],
        )
        .unwrap();
        let catalog_before = fs::read(&catalog.path).unwrap();
        let allocations_before = allocation_paths(&catalog.root);
        let error = kind
            .prepare(&catalog, &cut)
            .expect_err("native frozen bytes cannot depend on optional display evidence");
        assert!(!error.committed());
        assert_eq!(fs::read(&catalog.path).unwrap(), catalog_before, "{kind:?}");
        assert_eq!(
            allocation_paths(&catalog.root),
            allocations_before,
            "{kind:?}"
        );
        assert_eq!(store.load_canonical().unwrap(), cut.canonical);
    }
}

#[test]
fn clone_fork_binary_copies_survive_ordinary_source_session_deletion_and_cold_reopen() {
    use crate::local_runtime::session::deletion::SessionDeleteResult;
    for kind in [CopyKind::Clone, CopyKind::Fork] {
        let (root, mut catalog, _) = open_catalog();
        let (store, cut) = source(&catalog, true);
        let source_session = first_session(&catalog);
        let source_database = catalog.database_path(&source_session, &cut.conversation_id);
        let source_allocation = source_database.parent().unwrap().to_path_buf();
        let prepared = kind.prepare(&catalog, &cut).unwrap();
        kind.publish(&mut catalog, &prepared);
        drop(store);
        let SessionDeleteResult::Preview { preview } = catalog.delete_preview(&source_session)
        else {
            panic!("source deletion preview")
        };
        let work = catalog
            .commit_delete(&source_session, &preview.target_revision)
            .unwrap()
            .unwrap();
        let cleaned = work.run();
        assert!(matches!(
            catalog.finish_delete(&work.record, cleaned),
            SessionDeleteResult::Deleted { .. }
        ));
        assert!(
            !source_allocation.exists(),
            "ordinary deletion physically removes its private Conversation allocation"
        );
        drop(catalog);
        let reopened = reopen_catalog(root.path());
        let copied = store_for(&reopened, &prepared.session_id, &prepared.conversation_id);
        assert_original(&copied);
        assert!(reopened.snapshot(&source_session).is_err());
        assert!(reopened.snapshot(&prepared.session_id).is_ok());
    }
}
