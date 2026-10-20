//! Real SQLite writer interleavings and lifetime ownership, with explicit gates.
use crate::durable::SqliteConversationStore;
use crate::local_runtime::session::tests::{deletion_tests::child, open_catalog};
use crate::message::content::TextBlock;
use crate::message::types::{AssistantContentBlock, AssistantMessageBlock, MessageBlock};
use crate::runtime::identity::{ConversationId, MessageId, SessionId};
use crate::runtime::local_storage::{ConversationExclusion, ProductRoot};
use crate::session_archive::{SessionArchivePrepareError, SessionArchiveProducer, capture_probe};
use rustx::durable::{CanonicalStore, ProjectionStore};
use std::io::Read;
use std::path::PathBuf;
use std::sync::mpsc;
use tokio_util::sync::CancellationToken;

fn message(id: &str) -> MessageBlock {
    MessageBlock::Assistant(AssistantMessageBlock {
        id: MessageId::new(id),
        content: vec![AssistantContentBlock::Text(TextBlock { text: id.into() })],
    })
}

struct Fixture {
    root: tempfile::TempDir,
    session: SessionId,
    members: Vec<(ConversationId, PathBuf)>,
}
impl Fixture {
    fn new() -> Self {
        let (root, catalog, _) = open_catalog();
        let session = catalog.list_page(None, 0, 1).unwrap().sessions[0]
            .id
            .clone();
        let (node, _) = catalog.lineage(&session, None).unwrap();
        let database = catalog.database_path(&session, &node.conversation_id);
        let store = SqliteConversationStore::open(node.conversation_id.clone(), &database).unwrap();
        // macOS temporary roots can be /var aliases of canonical /private/var.
        let canonical_root = root.path().canonicalize().unwrap();
        let descendant = child(&canonical_root, &store, 1, false);
        let child_path = crate::runtime::subagent::child_conversation_store_path(
            &canonical_root,
            &session,
            &descendant,
        );
        let mut members = vec![(node.conversation_id, database), (descendant, child_path)];
        members.sort_by(|a, b| a.0.cmp(&b.0));
        Self {
            root,
            session,
            members,
        }
    }
    fn assert_released(&self) {
        let root = ProductRoot::existing(self.root.path()).unwrap();
        let _ownership = root.freeze_ownership().unwrap();
        for (_, path) in &self.members {
            let _exclusive = ConversationExclusion::acquire(&root, path.parent().unwrap()).unwrap();
            let connection = rusqlite::Connection::open(path).unwrap();
            connection.busy_timeout(std::time::Duration::ZERO).unwrap();
            connection
                .execute_batch("BEGIN IMMEDIATE; ROLLBACK")
                .unwrap();
        }
    }
}

#[test]
fn wal_archive_excludes_ordered_writes_across_conversations() {
    let fixture = Fixture::new();
    // Open all writer handles before capture. Neither catalog coordination nor
    // this process's archive objects can substitute for SQLite writer exclusion.
    let writers: Vec<_> = fixture
        .members
        .iter()
        .map(|(id, path)| SqliteConversationStore::open(id.clone(), path).unwrap())
        .collect();
    let (start, wait_start) = mpsc::channel();
    let (blocked, wait_blocked) = mpsc::channel();
    let (release, wait_release) = mpsc::channel();
    let writer = std::thread::spawn(move || {
        wait_start.recv().unwrap();
        let mut release = Some(wait_release);
        let _probe = crate::durable::sqlite::busy_probe::observe(move |_| {
            if let Some(release) = release.take() {
                blocked.send(()).unwrap();
                release.recv().unwrap();
            }
        });
        for (index, store) in writers.iter().enumerate() {
            store
                .append_canonical(&message(&format!("after-cut-{index}")))
                .unwrap();
        }
    });
    let first = fixture.members[0].0.clone();
    let _probe = capture_probe::observe(move |id| {
        if *id == first {
            start.send(()).unwrap();
            // The first commit is blocked by the already-captured first store.
            // Hence its later second-store commit cannot appear in this cut.
            wait_blocked.recv().unwrap();
        }
    });
    let cut = SessionArchiveProducer::prepare(
        fixture.root.path(),
        &fixture.session,
        &CancellationToken::new(),
    )
    .unwrap();
    release.send(()).unwrap();
    writer.join().unwrap();
    let mut bytes = std::io::Cursor::new(Vec::new());
    cut.write_test(&mut bytes).unwrap();
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes.into_inner())).unwrap();
    for (id, path) in &fixture.members {
        let mut messages = String::new();
        zip.by_name(&format!("sessions/{id}/messages.jsonl"))
            .unwrap()
            .read_to_string(&mut messages)
            .unwrap();
        assert!(messages.is_empty(), "torn cut: {messages}");
        let store = SqliteConversationStore::open_existing(id.clone(), path).unwrap();
        assert_eq!(store.load_head().unwrap().active_message_ids.len(), 1);
    }
    fixture.assert_released();
}

#[test]
fn wal_archive_partial_failure_releases_prior_barriers_and_pins() {
    let fixture = Fixture::new();
    let first = fixture.members[0].0.clone();
    let blocked = rusqlite::Connection::open(&fixture.members[1].1).unwrap();
    blocked.execute_batch("BEGIN IMMEDIATE").unwrap();
    let observed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let seen = observed.clone();
    let _probe = capture_probe::observe(move |id| {
        assert_eq!(*id, first);
        seen.store(true, std::sync::atomic::Ordering::SeqCst);
    });
    assert!(matches!(
        SessionArchiveProducer::prepare(
            fixture.root.path(),
            &fixture.session,
            &CancellationToken::new()
        ),
        Err(SessionArchivePrepareError::Storage)
    ));
    assert!(observed.load(std::sync::atomic::Ordering::SeqCst));
    blocked.execute_batch("ROLLBACK").unwrap();
    fixture.assert_released();
}

#[test]
fn wal_archive_cancel_after_first_frontier_releases_barriers_and_pins() {
    let fixture = Fixture::new();
    let cancel = CancellationToken::new();
    let signal = cancel.clone();
    let first = fixture.members[0].0.clone();
    let _probe = capture_probe::observe(move |id| {
        assert_eq!(*id, first);
        signal.cancel();
    });
    assert!(matches!(
        SessionArchiveProducer::prepare(fixture.root.path(), &fixture.session, &cancel),
        Err(SessionArchivePrepareError::Cancelled)
    ));
    fixture.assert_released();
}

#[test]
fn wal_fork_installs_frozen_uncheckpointed_source_and_survives_source_removal() {
    use crate::local_runtime::session::{LineageSide, SessionNodeOrigin};
    use crate::message::types::{InboundKind, UserContentBlock, UserMessageBlock, UserSource};
    let (_root, mut catalog, _) = open_catalog();
    let session = catalog.list_page(None, 0, 1).unwrap().sessions[0]
        .id
        .clone();
    let (node, state) = catalog.lineage(&session, None).unwrap();
    let database = catalog.database_path(&session, &node.conversation_id);
    let store = SqliteConversationStore::open(node.conversation_id.clone(), &database).unwrap();
    let before = MessageBlock::User(UserMessageBlock {
        id: MessageId::new("fork-before"),
        content: vec![UserContentBlock::Text(TextBlock {
            text: "frozen WAL input".into(),
        })],
        source: UserSource::Human,
        kind: InboundKind::Message,
        timestamp: None,
    });
    // Pin the pre-write snapshot so checkpointing cannot put the new row in
    // the main file. Fork must consume SQLite authority, never copy that file.
    let reader = rusqlite::Connection::open(&database).unwrap();
    reader
        .execute_batch("BEGIN; SELECT * FROM rustx_store")
        .unwrap();
    store.append_canonical(&before).unwrap();
    let response = message("fork-response");
    store.append_canonical(&response).unwrap();
    let revision = store.load_head().unwrap().revision;
    let cut = store.read_lineage_cut(revision).unwrap();
    let other = SqliteConversationStore::open(node.conversation_id.clone(), &database).unwrap();
    other.append_canonical(&message("fork-after-cut")).unwrap();
    let (prepared, _) = catalog
        .prepare_fork_session(&state, &cut, response.id(), LineageSide::After)
        .unwrap();
    catalog
        .publish_session(
            &prepared,
            SessionNodeOrigin::Fork {
                source_session: session,
                source_node: node.id,
                source_surface_revision: revision,
                source_message: response.id().clone(),
                source_locator: crate::observation::content::CanonicalContentLocator::ByMessageId {
                    message_id: response.id().clone(),
                },
                side: LineageSide::After,
            },
        )
        .unwrap();
    drop((store, other, reader, cut));
    // Deliberately remove the retired fixture allocation after its owners end.
    std::fs::remove_dir_all(database.parent().unwrap()).unwrap();
    let destination =
        SqliteConversationStore::open_existing(prepared.conversation_id, &prepared.database_path)
            .unwrap();
    let mut copied = destination.load_canonical().unwrap();
    let [MessageBlock::User(user), MessageBlock::Assistant(assistant)] = copied.as_mut_slice()
    else {
        panic!("Fork must contain exactly the captured user and response");
    };
    assert_ne!(&user.id, before.id());
    assert_ne!(&assistant.id, response.id());
    user.id = before.id().clone();
    assistant.id = response.id().clone();
    assert_eq!(copied, vec![before, response]);
}

#[test]
fn wal_existing_management_does_not_convert_and_writer_transition_validates_identity() {
    let root = tempfile::tempdir().unwrap();
    let path = root
        .path()
        .canonicalize()
        .unwrap()
        .join("conversation.sqlite");
    let id = ConversationId::generate();
    let store = SqliteConversationStore::open(id.clone(), &path).unwrap();
    store.initialize(&[]).unwrap();
    drop(store);
    let raw = rusqlite::Connection::open(&path).unwrap();
    raw.execute_batch("PRAGMA journal_mode=DELETE").unwrap();
    drop(raw);
    assert!(SqliteConversationStore::open_existing(id.clone(), &path).is_err());
    assert!(SqliteConversationStore::open(ConversationId::generate(), &path).is_err());
    assert_eq!(&std::fs::read(&path).unwrap()[18..20], &[1, 1]);
    let controller = std::sync::Arc::new(
        crate::runtime::local_storage::ProductController::acquire(root.path()).unwrap(),
    );
    let writer = SqliteConversationStore::recover_existing(id.clone(), &path, &controller).unwrap();
    assert_eq!(&std::fs::read(&path).unwrap()[18..20], &[2, 2]);
    writer
        .append_canonical(&message("recovered-writer"))
        .unwrap();
    let management = SqliteConversationStore::open_existing(id, &path).unwrap();
    assert_eq!(management.load_head().unwrap(), writer.load_head().unwrap());
    assert!(
        management
            .append_canonical(&message("forbidden-write"))
            .is_err()
    );
    assert_eq!(writer.load_head().unwrap().active_message_ids.len(), 1);
    assert!(
        rusqlite::version_number() >= 3_051_003,
        "bundled SQLite needs the WAL-reset fix"
    );
}

#[test]
fn wal_archive_preflight_validates_captured_head_after_later_activation() {
    use crate::scripted_suites::support::context::{checkpoint_commit_fixture, checkpoint_fixture};
    let root = tempfile::tempdir().unwrap();
    let path = root
        .path()
        .canonicalize()
        .unwrap()
        .join("conversation.sqlite");
    let id = ConversationId::generate();
    let writer = SqliteConversationStore::open(id.clone(), &path).unwrap();
    writer.initialize(&[message("original")]).unwrap();
    writer
        .commit_projection(checkpoint_commit_fixture(
            &writer,
            checkpoint_fixture(&writer, "captured", 1, "captured summary"),
            None,
        ))
        .unwrap();
    let archive = SqliteConversationStore::open_for_archive(id, &path).unwrap();
    archive.archive_barrier().unwrap();
    let (frontiers, read_cut) = archive.archive_frontiers().unwrap();
    archive.archive_release().unwrap();
    writer
        .commit_projection(checkpoint_commit_fixture(
            &writer,
            checkpoint_fixture(&writer, "later", 1, "later summary"),
            None,
        ))
        .unwrap();
    assert_ne!(
        writer.projected_history().unwrap().cut.checkpoint_id,
        frontiers.projection_head
    );
    archive.archive_validate(&frontiers, &read_cut).unwrap();
}
