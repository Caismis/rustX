//! Issue #387 deterministic acceptance regressions for the bounded
//! Conversation identity reservation contract.
//!
//! These tests exercise the real native allocation, preparation and
//! publication paths. They never fabricate a final catalog state and call it a
//! regression: fixtures are built through `prepare_session`/`publish_session`,
//! the same owners production uses. The only synchronization primitives are
//! barriers and process gates; no test depends on a sleep.
//!
//! Requirement mapping (R01-R14) lives in `docs/durable-sessions.md` and the
//! delivery report. Process-death R05 lives in `runtime::local_storage::tests`
//! because it must fork the real test binary.

use super::super::conversation_database_path;
use super::*;
use crate::durable::conversation_store_opens_on_this_thread;
use crate::local_runtime::session::LineageSide;
use crate::runtime::identity::UuidV7Generator;
use crate::runtime::local_storage::{ProductController, ProductRoot};
use std::collections::VecDeque;
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Barrier, Mutex};

#[derive(Debug)]
struct Sequence(Mutex<VecDeque<uuid::Uuid>>);

impl UuidV7Generator for Sequence {
    fn next_uuid(&self) -> uuid::Uuid {
        let mut values = self.0.lock().unwrap();
        if values.len() > 1 {
            values.pop_front().unwrap()
        } else {
            *values.front().unwrap()
        }
    }
}

fn uuid(value: u64) -> uuid::Uuid {
    uuid::Uuid::parse_str(&format!("01900000-0000-7000-8000-{value:012}")).unwrap()
}

fn identities(values: &[u64]) -> Arc<dyn UuidV7Generator> {
    Arc::new(Sequence(Mutex::new(
        values.iter().map(|value| uuid(*value)).collect(),
    )))
}

fn template() -> SessionPersistentState {
    SessionPersistentState {
        cwd: PathBuf::from("/workspace"),
        model: None,
    }
}

fn assistant(id: &str, value: &str) -> MessageBlock {
    MessageBlock::Assistant(AssistantMessageBlock {
        id: MessageId::new(id),
        content: vec![AssistantContentBlock::Text(TextBlock {
            text: value.to_owned(),
        })],
    })
}

fn marker(root: &Path, conversation: &ConversationId) -> PathBuf {
    root.join("conversation-reservations")
        .join(conversation.as_str())
}

/// Build `count` real published Sessions. Allocations are created by the real
/// `prepare_session` owner (real Session directories, Conversation stores and
/// reservation markers). The published catalog document is written by the
/// catalog's real `commit` publication owner in one batch: `publish_session`
/// would serialize the whole growing document once per Session, which is the
/// O(n^2) cost the benchmark measures, and would dominate this correctness
/// fixture without changing what is published. Fixture construction is
/// deliberately outside the measured reservation operation.
fn seed_published_sessions(catalog: &mut SessionCatalog, count: usize) {
    use super::super::{PersistedSession, SessionNode};
    let mut prepared = Vec::with_capacity(count);
    for _ in 0..count {
        let lineage = catalog.prepare_session(&template(), &[]).unwrap();
        // The same destination-completeness check `build_session_document`
        // performs before it publishes a Session.
        assert!(lineage.database_path.is_file());
        prepared.push(lineage);
    }
    let mut next = catalog.document.clone();
    for lineage in prepared {
        let now = chrono::Utc::now();
        let node = SessionNode {
            ordinal: next.next_node_ordinal,
            id: lineage.node_id.clone(),
            parent: None,
            conversation_id: lineage.conversation_id.clone(),
            origin: SessionNodeOrigin::New,
        };
        let mut nodes = std::collections::BTreeMap::new();
        nodes.insert(lineage.node_id.clone(), node);
        next.sessions.insert(
            lineage.session_id.clone(),
            PersistedSession {
                uploads: lineage.uploads.clone(),
                ordinal: next.next_session_ordinal,
                id: lineage.session_id.clone(),
                name: None,
                display_preview: lineage.display_preview.clone(),
                created_at: now,
                updated_at: now,
                active_node: lineage.node_id.clone(),
                nodes,
                state: lineage.state.clone(),
                settings_revision: 0,
            },
        );
        next.next_session_ordinal += 1;
        next.next_node_ordinal += 1;
    }
    catalog.commit(next).unwrap();
}

fn set_directory_mode(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
}

/// A real filesystem-inspection probe: watch the existing `sessions/`
/// directory for directory access/opens. `readdir` on a directory emits
/// `IN_ACCESS` on it, so any alternate enumeration path that walks the
/// existing Session tree is observed at the kernel boundary. Linux-only;
/// other platforms rely on the execute-only fixture below.
#[cfg(target_os = "linux")]
struct SessionsEnumerationProbe {
    watch: nix::sys::inotify::Inotify,
    descriptor: nix::sys::inotify::WatchDescriptor,
    accesses: u64,
}

#[cfg(target_os = "linux")]
impl SessionsEnumerationProbe {
    fn install(sessions: &Path) -> Self {
        use nix::sys::inotify::{AddWatchFlags as F, InitFlags, Inotify};
        let watch = Inotify::init(InitFlags::IN_NONBLOCK | InitFlags::IN_CLOEXEC).unwrap();
        let descriptor = watch
            .add_watch(sessions, F::IN_ACCESS | F::IN_OPEN)
            .unwrap();
        let mut probe = Self {
            watch,
            descriptor,
            accesses: 0,
        };
        probe.drain();
        probe
    }

    fn drain(&mut self) {
        use nix::sys::inotify::AddWatchFlags as F;
        loop {
            match self.watch.read_events() {
                Ok(events) => {
                    for event in events {
                        if event.wd == self.descriptor
                            && event.mask.intersects(F::IN_ACCESS | F::IN_OPEN)
                        {
                            self.accesses += 1;
                        }
                    }
                }
                Err(nix::errno::Errno::EAGAIN) => return,
                Err(error) => panic!("session enumeration probe read: {error}"),
            }
        }
    }
}

/// R01: with 1 and 1000 existing **published** Sessions, a new Conversation
/// reservation enumerates zero existing Session directories.
///
/// The fixture is real published Session state built through the production
/// owners. The measured reservation runs against an OS-level fixture where
/// direct known-path access still works but directory enumeration is
/// observable (inotify `IN_ACCESS`/`IN_OPEN` on Linux) and denied
/// (execute-only mode), so an alternate traversal path fails or is detected at
/// the actual filesystem-inspection boundary rather than by counting a
/// particular helper.
#[test]
fn r01_reservation_inspects_zero_existing_session_directories() {
    for sessions in [1_usize, 1000] {
        let directory = tempfile::tempdir().unwrap();
        let controller = ProductController::acquire(directory.path()).unwrap();
        let mut catalog = SessionCatalog::empty(&controller).unwrap();
        seed_published_sessions(&mut catalog, sessions);
        drop(catalog);

        let sessions_root = directory.path().join("sessions");
        // `sessions/` holds the `catalog.json` plus one directory per Session.
        assert_eq!(
            std::fs::read_dir(&sessions_root).unwrap().count(),
            sessions + 1,
            "fixture did not publish {sessions} Sessions"
        );
        #[cfg(target_os = "linux")]
        let mut probe = SessionsEnumerationProbe::install(&sessions_root);
        // Deny directory enumeration while preserving direct known-path access.
        set_directory_mode(&sessions_root, 0o111);
        #[cfg(target_os = "linux")]
        probe.drain();

        // The operation result itself is the proof: exactly one reservation of
        // this identity succeeds and returns `Ok`. No process-global counter is
        // read, so unrelated parallel tests cannot affect the assertion.
        let fresh = ConversationId::generate();
        controller.reserve_conversation(&fresh).unwrap();

        #[cfg(target_os = "linux")]
        {
            probe.drain();
            assert_eq!(
                probe.accesses, 0,
                "reservation accessed/enumerated the existing Session tree with {sessions} present"
            );
        }
        // Positive control: for an ordinary user the fixture really does deny
        // directory enumeration; root bypasses mode bits, so the inotify probe
        // above remains the detection mechanism there.
        if !nix::unistd::geteuid().is_root() {
            assert!(
                std::fs::read_dir(&sessions_root).is_err(),
                "execute-only fixture did not deny enumeration"
            );
        }
        set_directory_mode(&sessions_root, 0o755);
        assert!(marker(directory.path(), &fresh).is_file());
    }
}

/// R02: injecting the same identity twice has exactly one winner; the other
/// rejects without overwriting the first reservation.
#[test]
fn r02_duplicate_identity_rejects_without_overwriting() {
    let directory = tempfile::tempdir().unwrap();
    let controller = ProductController::acquire(directory.path()).unwrap();
    let conversation = ConversationId::from_uuid(uuid(7)).unwrap();
    controller.reserve_conversation(&conversation).unwrap();
    let path = marker(directory.path(), &conversation);
    let retained = std::fs::read(&path).unwrap();
    assert_eq!(
        controller
            .reserve_conversation(&conversation)
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::AlreadyExists
    );
    assert_eq!(std::fs::read(&path).unwrap(), retained);
}

/// R03: concurrent reservations of the same identity have exactly one
/// exclusive winner at the create-new linearization point. The production
/// allocation plus allocation-directory creation is exercised on both sides.
#[test]
fn r03_concurrent_same_identity_has_one_winner() {
    let directory = tempfile::tempdir().unwrap();
    let controller = ProductController::acquire(directory.path()).unwrap();
    let conversation = ConversationId::from_uuid(uuid(11)).unwrap();
    let barrier = Barrier::new(2);
    let results = std::thread::scope(|scope| {
        let workers: Vec<_> = [30, 40]
            .into_iter()
            .map(|value| {
                let barrier = &barrier;
                let product = &*controller;
                let conversation = &conversation;
                scope.spawn(move || {
                    let session = SessionId::from_uuid(uuid(value)).unwrap();
                    let allocation = conversation_database_path(
                        &product.root().join("sessions"),
                        &session,
                        conversation,
                    )
                    .parent()
                    .unwrap()
                    .to_owned();
                    barrier.wait();
                    product.reserve_conversation(conversation).and_then(|_| {
                        SessionCatalog::create_conversation_allocation(product, &allocation)
                    })
                })
            })
            .collect();
        workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .collect::<Vec<_>>()
    });
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|result| result
                .as_ref()
                .is_err_and(|error| error.kind() == std::io::ErrorKind::AlreadyExists))
            .count(),
        1
    );
}

/// R06/R09: a prepared but unpublished Conversation is an inert orphan. Its
/// bytes and its reservation exist, but they establish no Session membership
/// and authorize no execution, across a reopen.
#[test]
fn r06_prepared_unpublished_is_inert_across_reopen() {
    let directory = tempfile::tempdir().unwrap();
    let controller = ProductController::acquire(directory.path()).unwrap();
    let catalog = SessionCatalog::empty(&controller).unwrap();
    let prepared = catalog.prepare_session(&template(), &[]).unwrap();
    assert!(prepared.database_path.is_file());
    assert!(marker(directory.path(), &prepared.conversation_id).is_file());
    assert!(catalog.list_page(None, 0, 32).unwrap().sessions.is_empty());
    assert!(catalog.snapshot(&prepared.session_id).is_err());
    drop(catalog);
    let reopened = SessionCatalog::open_existing(directory.path())
        .unwrap()
        .unwrap();
    assert!(reopened.list_page(None, 0, 32).unwrap().sessions.is_empty());
    // Existence never claims membership; the consumed identity stays consumed.
    assert!(
        controller
            .reserve_conversation(&prepared.conversation_id)
            .is_err()
    );
}

/// R09: a reservation marker and a populated allocation directory alone can
/// never claim ownership or authorize execution. Only a Catalog commit names a
/// destination.
#[test]
fn r09_marker_and_bytes_alone_never_claim_ownership() {
    let directory = tempfile::tempdir().unwrap();
    let controller = ProductController::acquire(directory.path()).unwrap();
    let catalog = SessionCatalog::empty(&controller).unwrap();
    let conversation = ConversationId::generate();
    let session = SessionId::generate();
    controller.reserve_conversation(&conversation).unwrap();
    let allocation = directory
        .path()
        .join("sessions")
        .join(session.as_str())
        .join("conversations")
        .join(conversation.as_str());
    std::fs::create_dir_all(&allocation).unwrap();
    std::fs::write(allocation.join("conversation.sqlite"), b"inert").unwrap();
    // Storage bytes may be openable as an inert allocation, but no native
    // authority names them: no Session membership, no resumable target and no
    // execution. Only a Catalog commit can claim the orphan.
    assert!(catalog.acquire_session(&session, None).is_err());
    assert!(catalog.list_page(None, 0, 32).unwrap().sessions.is_empty());
    assert!(catalog.snapshot(&session).is_err());
    assert!(allocation.join("conversation.sqlite").is_file());
}

/// R10: an unsupported old populated layout is rejected before allocation, at
/// both the root and the ordinary controller entry point that child/subagent
/// allocation shares. No fallback scan, no reinterpretation, no data deletion.
#[test]
fn r10_old_populated_layout_is_rejected_before_allocation() {
    let directory = tempfile::tempdir().unwrap();
    let session = SessionId::generate();
    let conversation = ConversationId::generate();
    let allocation = directory
        .path()
        .join("sessions")
        .join(session.as_str())
        .join("conversations")
        .join(conversation.as_str());
    std::fs::create_dir_all(&allocation).unwrap();
    let durable = allocation.join("conversation.sqlite");
    std::fs::write(&durable, b"old durable bytes").unwrap();
    assert!(ProductRoot::existing(directory.path()).is_err());
    assert!(ProductRoot::create(directory.path()).is_err());
    assert!(
        crate::local_runtime::session_controller::SessionController::open(directory.path())
            .is_err()
    );
    assert_eq!(std::fs::read(&durable).unwrap(), b"old durable bytes");
    assert!(!directory.path().join("conversation-reservations").exists());
}

/// R11: fresh supported root initialization and restart preserve consumed
/// identities and reissue only genuinely fresh ones.
#[test]
fn r11_reopen_preserves_consumed_identities() {
    let directory = tempfile::tempdir().unwrap();
    let consumed = ConversationId::from_uuid(uuid(50)).unwrap();
    {
        let controller = ProductController::acquire(directory.path()).unwrap();
        controller.reserve_conversation(&consumed).unwrap();
        assert!(directory.path().join("conversation-reservations").is_dir());
    }
    let reopened = ProductRoot::existing(directory.path()).unwrap();
    assert_eq!(
        reopened.reserve_conversation(&consumed).unwrap_err().kind(),
        std::io::ErrorKind::AlreadyExists
    );
    assert!(
        reopened
            .reserve_conversation(&ConversationId::from_uuid(uuid(51)).unwrap())
            .is_ok()
    );
}

/// R08: every allocation path — new Session, clone, fork, and branch node —
/// shares the same reservation contract and leaves a consumed marker. The
/// fixture is a real published Session with real canonical history.
#[test]
fn r08_all_allocation_paths_share_the_reservation_contract() {
    let directory = tempfile::tempdir().unwrap();
    let controller = ProductController::acquire(directory.path()).unwrap();
    let mut catalog = SessionCatalog::empty(&controller).unwrap();
    let root = catalog.prepare_session(&template(), &[]).unwrap();
    catalog
        .publish_session(&root, SessionNodeOrigin::New)
        .unwrap();
    let history = vec![user("r08-user-a", "A"), assistant("r08-assistant-a", "B")];
    let (conversation, session, _) = append_history(&catalog, &history);
    let store = store_for(&catalog, &session, &conversation);
    let source = lineage_at(&store, &conversation, store.load_head().unwrap().revision);

    let cloned = catalog.prepare_clone_session(&state(), &source).unwrap();
    let (forked, _) = catalog
        .prepare_fork_session(
            &state(),
            &source,
            &MessageId::new("r08-assistant-a"),
            LineageSide::After,
        )
        .unwrap();
    let (branched, _) = catalog
        .prepare_tree_node(
            &session,
            &state(),
            &source,
            &MessageId::new("r08-assistant-a"),
            LineageSide::After,
        )
        .unwrap();
    let fresh = catalog.prepare_session(&template(), &[]).unwrap();

    for prepared in [&cloned, &forked, &branched, &fresh] {
        assert!(
            marker(directory.path(), &prepared.conversation_id).is_file(),
            "allocation path did not reserve {}",
            prepared.conversation_id
        );
        assert!(
            controller
                .reserve_conversation(&prepared.conversation_id)
                .is_err(),
            "allocation path left {} reusable",
            prepared.conversation_id
        );
    }
    assert_eq!(branched.session_id, session);
    assert_ne!(cloned.session_id, session);
    assert_ne!(forked.session_id, session);
}

/// R13: listing and searching still open zero conversation stores even after
/// reservation markers and prepared allocations exist, and a prepared
/// clone/fork publication carries its seed-derived preview.
#[test]
fn r13_list_and_search_open_zero_stores_and_clone_keeps_preview() {
    let directory = tempfile::tempdir().unwrap();
    let controller = ProductController::acquire(directory.path()).unwrap();
    let mut catalog = SessionCatalog::empty(&controller).unwrap();
    let root = catalog.prepare_session(&template(), &[]).unwrap();
    catalog
        .publish_session(&root, SessionNodeOrigin::New)
        .unwrap();
    let history = vec![
        user("r13-user-a", "the seed subject line"),
        assistant("r13-assistant-a", "answer"),
    ];
    let (conversation, session, _) = append_history(&catalog, &history);
    let store = store_for(&catalog, &session, &conversation);
    let source = lineage_at(&store, &conversation, store.load_head().unwrap().revision);
    let cloned = catalog.prepare_clone_session(&state(), &source).unwrap();
    assert_eq!(
        cloned.display_preview.as_deref(),
        Some("the seed subject line")
    );
    catalog
        .publish_session(
            &cloned,
            SessionNodeOrigin::Clone {
                source_session: session.clone(),
                source_node: root.node_id.clone(),
                source_surface_revision: source.surface_revision,
            },
        )
        .unwrap();

    let opens_before = conversation_store_opens_on_this_thread();
    let page = catalog.list_page(None, 0, 32).unwrap();
    assert_eq!(page.sessions.len(), 2);
    let searched = catalog.list_page(Some("seed subject"), 0, 32).unwrap();
    assert_eq!(searched.sessions.len(), 1);
    let opens = conversation_store_opens_on_this_thread() - opens_before;
    assert_eq!(opens, 0, "list/search opened ConversationStores");
}

/// The child side of the R06/R07 process-death regressions: prepare (and
/// optionally publish) through the real owners with fixed identities, print
/// the resulting identities, and park so the parent can SIGKILL a process at a
/// proven boundary.
#[test]
fn reservation_create_process_gate() {
    let Some(root) = std::env::var_os("RUSTX_387_CREATE_ROOT") else {
        return;
    };
    let mode = std::env::var("RUSTX_387_CREATE_MODE").unwrap();
    let controller = ProductController::acquire(Path::new(&root)).unwrap();
    let mut catalog = SessionCatalog::empty(&controller)
        .unwrap()
        .with_identity_generator(identities(&[900, 901, 902]));
    let prepared = catalog.prepare_session(&template(), &[]).unwrap();
    if mode == "publish" {
        catalog
            .publish_session(&prepared, SessionNodeOrigin::New)
            .unwrap();
        println!(
            "PUBLISHED {} {}",
            prepared.session_id, prepared.conversation_id
        );
    } else {
        println!(
            "PREPARED {} {}",
            prepared.session_id, prepared.conversation_id
        );
    }
    std::io::stdout().flush().unwrap();
    std::io::stdin().read_exact(&mut [0]).unwrap();
}

/// Spawns the process gate against `root` and returns the unreaped child plus
/// the identities it announced. The wait is a token rendezvous, never a sleep.
/// The caller owns the child and must kill and reap it.
#[allow(clippy::zombie_processes)]
fn spawn_create_child(root: &Path, mode: &str) -> (std::process::Child, SessionId, ConversationId) {
    use std::io::{BufRead, BufReader};
    let mut process = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "local_runtime::session::tests::reservation_tests::reservation_create_process_gate",
            "--nocapture",
        ])
        .env("RUSTX_387_CREATE_ROOT", root)
        .env("RUSTX_387_CREATE_MODE", mode)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut output = BufReader::new(process.stdout.take().unwrap());
    loop {
        let mut line = String::new();
        assert_ne!(
            output.read_line(&mut line).unwrap(),
            0,
            "create gate child exited before announcing its boundary"
        );
        let line = line.trim();
        let rest = line
            .strip_prefix("PREPARED ")
            .or_else(|| line.strip_prefix("PUBLISHED "));
        if let Some(rest) = rest {
            let mut parts = rest.split_whitespace();
            let session = SessionId::parse(parts.next().unwrap()).unwrap();
            let conversation = ConversationId::parse(parts.next().unwrap()).unwrap();
            process.stdout = Some(output.into_inner());
            return (process, session, conversation);
        }
    }
}

/// R06: real process death after initialization and before publication leaves
/// no half-visible Session; the prepared Conversation bytes remain an inert
/// orphan and the consumed identity cannot be reallocated after restart.
#[test]
fn r06_kill_after_preparation_before_publication_leaves_inert_orphan() {
    let directory = tempfile::tempdir().unwrap();
    let (mut child, session, conversation) = spawn_create_child(directory.path(), "prepare");
    child.kill().unwrap();
    child.wait().unwrap();
    let reopened = SessionCatalog::open_existing(directory.path())
        .unwrap()
        .unwrap();
    assert!(reopened.list_page(None, 0, 32).unwrap().sessions.is_empty());
    assert!(reopened.snapshot(&session).is_err());
    let database = reopened.database_path(&session, &conversation);
    assert!(
        database.is_file(),
        "prepared bytes must remain as inert residue"
    );
    let controller = ProductController::acquire(directory.path()).unwrap();
    assert!(controller.reserve_conversation(&conversation).is_err());
}

/// R07: real process death after publication leaves a published graph naming a
/// completely prepared, valid Conversation.
#[test]
fn r07_kill_after_publication_leaves_complete_valid_conversation() {
    let directory = tempfile::tempdir().unwrap();
    let (mut child, session, conversation) = spawn_create_child(directory.path(), "publish");
    child.kill().unwrap();
    child.wait().unwrap();
    let reopened = SessionCatalog::open_existing(directory.path())
        .unwrap()
        .unwrap();
    let page = reopened.list_page(None, 0, 32).unwrap();
    assert_eq!(page.sessions.len(), 1);
    assert_eq!(page.sessions[0].id, session);
    let (node, _) = reopened.lineage(&session, None).unwrap();
    assert_eq!(node.conversation_id, conversation);
    let store = SqliteConversationStore::open_existing(
        conversation.clone(),
        &reopened.database_path(&session, &conversation),
    )
    .unwrap();
    assert!(store.load_canonical().unwrap().is_empty());
}

/// Instrumented exclusive stage profile of the **real**
/// `SessionController::create_session` pipeline (Issue #387). This is a
/// separate tracing run, not a timing-threshold test: it prints exclusive
/// leaf-stage times, the measured total, and the unattributed remainder so the
/// delivery report can attribute cost to the real owners without making CI
/// depend on a duration. Inclusive parent operations (`prepare_session`,
/// `publish_session`) are never summed with their children.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "instrumented stage profile; run explicitly with --ignored --nocapture"]
#[allow(clippy::too_many_lines)]
async fn stage_profile_real_create_pipeline() {
    use crate::local_runtime::session::create_profile;
    use crate::local_runtime::session_controller::SessionController;
    const CREATES: u64 = 50;
    let existing: usize = std::env::var("RUSTX_387_PROFILE_EXISTING")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(0);
    let directory = tempfile::tempdir().unwrap();
    let controller = SessionController::open(directory.path()).unwrap();
    let template = template();
    for _ in 0..existing {
        controller.create_session(template.clone()).await.unwrap();
    }
    create_profile::enable();
    let mut measured_total = std::time::Duration::ZERO;
    for _ in 0..CREATES {
        let started = std::time::Instant::now();
        controller.create_session(template.clone()).await.unwrap();
        measured_total += started.elapsed();
        create_profile::count_create();
    }
    let times = create_profile::snapshot();
    create_profile::disable();
    let per_create = |value: u64| value / CREATES;
    let total_ns = u64::try_from(measured_total.as_nanos()).unwrap_or(u64::MAX) / CREATES;
    let exclusive_ns = times.exclusive_total_ns() / CREATES;
    let report = serde_json::json!({
        "creates": CREATES,
        "existing_sessions": existing,
        "path": "SessionController::create_session",
        "catalog_snapshot_ns": per_create(times.catalog_snapshot_ns),
        "reserve_ns": per_create(times.reserve_ns),
        "allocation_dir_ns": per_create(times.allocation_dir_ns),
        "sqlite_open_ns": per_create(times.sqlite_open_ns),
        "schema_and_seed_ns": per_create(times.schema_and_seed_ns),
        "catalog_document_clone_ns": per_create(times.catalog_document_clone_ns),
        "catalog_serialize_ns": per_create(times.catalog_serialize_ns),
        "temp_write_ns": per_create(times.temp_write_ns),
        "file_fsync_ns": per_create(times.file_fsync_ns),
        "rename_ns": per_create(times.rename_ns),
        "dir_fsync_ns": per_create(times.dir_fsync_ns),
        "measured_total_ns": total_ns,
        "exclusive_sum_ns": exclusive_ns,
        "unattributed_ns": total_ns.saturating_sub(exclusive_ns),
        "note": "exclusive leaf stages only; inclusive prepare/publish are not summed",
    });
    println!("STAGE_PROFILE {}", serde_json::to_string(&report).unwrap());
}

/// R06/R12 companion: a deterministic catalog fault before the visibility
/// rename never reports a visible Session, and the prepared destination stays
/// inert. The fault is the real catalog write owner's tested seam.
#[test]
fn r12_pre_visibility_fault_never_reports_success() {
    let directory = tempfile::tempdir().unwrap();
    let controller = ProductController::acquire(directory.path()).unwrap();
    let mut catalog = SessionCatalog::empty(&controller).unwrap();
    let prepared = catalog.prepare_session(&template(), &[]).unwrap();
    catalog.arm_write_fault_before_rename();
    let result = catalog.publish_session(&prepared, SessionNodeOrigin::New);
    assert!(result.is_err());
    assert!(!result.unwrap_err().committed());
    assert!(catalog.list_page(None, 0, 32).unwrap().sessions.is_empty());
    let reopened = SessionCatalog::open_existing(directory.path())
        .unwrap()
        .unwrap();
    assert!(reopened.list_page(None, 0, 32).unwrap().sessions.is_empty());
}

/// R12: reservation succeeds, then Conversation initialization fails. No
/// Session or node becomes visible, success is not reported, the reserved
/// identity stays consumed, the residue is inert across reopen, and a retry
/// selects a fresh identity rather than reusing the consumed one.
#[test]
fn r12_initialization_failure_after_reservation_is_inert() {
    let directory = tempfile::tempdir().unwrap();
    let controller = ProductController::acquire(directory.path()).unwrap();
    let mut catalog = SessionCatalog::empty(&controller)
        .unwrap()
        .with_identity_generator(identities(&[70, 71, 72, 73, 74, 75]));
    crate::local_runtime::session::fail_next_database_initializations(1);
    let error = catalog.prepare_session(&template(), &[]).unwrap_err();
    assert!(matches!(error, SessionError::Io { .. }), "{error:?}");
    assert!(catalog.list_page(None, 0, 32).unwrap().sessions.is_empty());
    // `allocate_ids` consumed session=70, node=71, conversation=72; the
    // marker for 72 is the retained consumption proof.
    let consumed = ConversationId::from_uuid(uuid(72)).unwrap();
    assert_eq!(
        controller
            .reserve_conversation(&consumed)
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::AlreadyExists
    );
    let reopened = SessionCatalog::open_existing(directory.path())
        .unwrap()
        .unwrap();
    assert!(reopened.list_page(None, 0, 32).unwrap().sessions.is_empty());
    drop(reopened);
    // Retry resumes from a fresh identity and publishes a complete Session.
    let prepared = catalog.prepare_session(&template(), &[]).unwrap();
    assert_ne!(prepared.conversation_id, consumed);
    catalog
        .publish_session(&prepared, SessionNodeOrigin::New)
        .unwrap();
    assert_eq!(catalog.list_page(None, 0, 32).unwrap().sessions.len(), 1);
}

/// R12: reservation and initialization both succeed, then destination
/// validation fails at publication. The Session never becomes visible, the
/// consumed identity stays consumed, the residue is inert, and a retry with a
/// fresh identity succeeds.
#[test]
fn r12_destination_validation_failure_after_initialization_is_inert() {
    let directory = tempfile::tempdir().unwrap();
    let controller = ProductController::acquire(directory.path()).unwrap();
    let mut catalog = SessionCatalog::empty(&controller).unwrap();
    let prepared = catalog.prepare_session(&template(), &[]).unwrap();
    assert!(prepared.database_path.is_file());
    // The prepared destination is complete at preparation time; make the
    // publication-time destination validation fail by removing the database.
    std::fs::remove_file(&prepared.database_path).unwrap();
    let error = catalog
        .publish_session(&prepared, SessionNodeOrigin::New)
        .unwrap_err();
    assert!(!error.committed());
    assert!(catalog.list_page(None, 0, 32).unwrap().sessions.is_empty());
    assert_eq!(
        controller
            .reserve_conversation(&prepared.conversation_id)
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::AlreadyExists
    );
    let reopened = SessionCatalog::open_existing(directory.path())
        .unwrap()
        .unwrap();
    assert!(reopened.list_page(None, 0, 32).unwrap().sessions.is_empty());
    drop(reopened);
    let next = catalog.prepare_session(&template(), &[]).unwrap();
    assert_ne!(next.conversation_id, prepared.conversation_id);
    catalog
        .publish_session(&next, SessionNodeOrigin::New)
        .unwrap();
    assert_eq!(catalog.list_page(None, 0, 32).unwrap().sessions.len(), 1);
}
