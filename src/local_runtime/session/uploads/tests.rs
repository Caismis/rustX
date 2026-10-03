#![allow(clippy::too_many_lines)] // Each test follows one durable lifecycle.
use super::*;
use crate::durable::{ConversationStore, SqliteConversationStore};
use crate::local_runtime::session::{SessionPersistentState, deletion::SessionDeleteResult};
use crate::local_runtime::session_controller::SessionController;
use crate::message::types::{
    InboundKind, MessageBlock, UserContentBlock, UserMessageBlock, UserSource,
};
use crate::model::uploads::UploadProjectionResolver;
use crate::runtime::conversation_runtime::Gate;
use crate::runtime::identity::MessageId;
use std::cell::RefCell;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::sync::Arc;

type Checkpoint =
    Arc<std::sync::Mutex<Option<(&'static str, Box<dyn FnOnce() -> io::Result<()> + Send>)>>>;
thread_local! {
    static CHECKPOINT: RefCell<Option<Checkpoint>> = const { RefCell::new(None) };
}
pub(crate) fn capture_checkpoint() -> Option<Checkpoint> {
    CHECKPOINT.with(|slot| slot.borrow().clone())
}
pub(crate) fn with_checkpoint<T>(checkpoint: Option<Checkpoint>, action: impl FnOnce() -> T) -> T {
    struct Clear;
    impl Drop for Clear {
        fn drop(&mut self) {
            CHECKPOINT.with(|slot| {
                slot.take();
            });
        }
    }
    CHECKPOINT.with(|slot| assert!(slot.replace(checkpoint).is_none()));
    let _clear = Clear;
    action()
}
pub(super) fn materialize_checkpoint(stage: &str) -> io::Result<()> {
    let armed = CHECKPOINT.with(|slot| {
        let slot = slot.borrow();
        let mut checkpoint = slot.as_ref()?.lock().unwrap();
        if checkpoint
            .as_ref()
            .is_some_and(|(armed, _)| *armed == stage)
        {
            checkpoint.take()
        } else {
            None
        }
    });
    armed.map_or(Ok(()), |(_, action)| action())
}
fn arm_checkpoint(stage: &'static str, action: impl FnOnce() -> io::Result<()> + Send + 'static) {
    CHECKPOINT.with(|slot| {
        assert!(
            slot.replace(Some(Arc::new(std::sync::Mutex::new(Some((
                stage,
                Box::new(action)
            ))))))
            .is_none()
        );
    });
}
fn checkpoint_consumed() -> bool {
    CHECKPOINT.with(|slot| {
        slot.take()
            .is_none_or(|checkpoint| checkpoint.lock().unwrap().is_none())
    })
}
fn tempdir() -> std::io::Result<tempfile::TempDir> {
    tempfile::tempdir_in(std::env::temp_dir().canonicalize()?)
}
async fn pending_destination(controller: &SessionController) -> SessionId {
    let catalog = controller.catalog.lock().await;
    assert_eq!(catalog.document.upload_preparations.len(), 1);
    catalog
        .document
        .upload_preparations
        .keys()
        .next()
        .unwrap()
        .clone()
}
fn file(name: &str, bytes: &[u8]) -> UploadFile {
    UploadFile {
        name: name.into(),
        bytes: bytes.into(),
    }
}
fn settings(path: &Path) -> SessionPersistentState {
    SessionPersistentState::from_input(
        &crate::local_runtime::configuration::SessionConfigInput::new(path.to_path_buf()),
    )
}
fn user(id: &str, refs: &[UploadedFile]) -> MessageBlock {
    MessageBlock::User(UserMessageBlock {
        id: MessageId::new(id),
        source: UserSource::Human,
        kind: InboundKind::Message,
        timestamp: None,
        content: refs
            .iter()
            .map(|f| UserContentBlock::UploadedFile(f.file.clone()))
            .chain([UserContentBlock::Text(crate::message::content::TextBlock {
                text: "Body\n unchanged  ".into(),
            })])
            .collect(),
    })
}
async fn delete(controller: &SessionController, id: &SessionId) {
    let SessionDeleteResult::Preview { preview } = controller.delete_preview(id).await else {
        panic!("preview");
    };
    assert!(matches!(
        controller
            .delete_session(id, &preview.target_revision)
            .await
            .unwrap(),
        SessionDeleteResult::Deleted { .. }
    ));
}
#[test]
fn names_and_symlink_ancestors_fail_closed() {
    for name in [
        "", ".", "..", "/abs", "a/b", "a\\b", "C:\\x", "nul", "CON.txt", "x\0y", "x.", "x ",
    ] {
        assert!(validate_name(name).is_err(), "{name:?}");
    }
    let workspace = tempdir().unwrap();
    let outside = tempdir().unwrap();
    std::fs::write(outside.path().join("keep"), b"untouched").unwrap();
    std::os::unix::fs::symlink(outside.path(), workspace.path().join(".agents")).unwrap();
    let mut registry = UploadRegistry::default();
    let inputs = vec![file("valid.txt", b"not outside")];
    let batch = registry
        .claim(workspace.path().canonicalize().unwrap(), &inputs)
        .unwrap();
    assert!(
        registry
            .materialize(
                &SessionId::new("ses_84097828-fc31-78c8-8292-10df48901a85"),
                &batch,
                &inputs
            )
            .is_err()
    );
    assert_eq!(std::fs::read_dir(outside.path()).unwrap().count(), 1);
    assert!(
        cleanup(
            workspace.path(),
            &SessionId::new("ses_84097828-fc31-78c8-8292-10df48901a85")
        )
        .is_err()
    );
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn commit_gate_receipts_restart_concurrency_and_failed_turn_are_independent() {
    let root = tempdir().unwrap();
    let workspace = tempdir().unwrap();
    let controller = SessionController::open(root.path()).unwrap();
    let a = controller
        .create_session(settings(workspace.path()))
        .await
        .unwrap()
        .session;
    let b = controller
        .create_session(settings(workspace.path()))
        .await
        .unwrap()
        .session;
    let gate = Arc::new(Gate::default());
    let release = gate.arm_scoped();
    *controller.upload_commit_gate.lock().unwrap() = Some(gate.clone());
    let worker = controller.clone();
    let id = a.id.clone();
    let upload = tokio::spawn(async move {
        worker
            .upload(
                &id,
                None,
                vec![file("a.txt", b"first"), file("b.png", b"second")],
            )
            .await
    });
    tokio::task::spawn_blocking(move || gate.wait_entered())
        .await
        .unwrap();
    assert!(!upload.is_finished());
    let registry = controller
        .catalog
        .lock()
        .await
        .upload_registry(&a.id)
        .unwrap();
    let (batch, allocation) = registry.allocations.iter().next().unwrap();
    assert!(!allocation.ready);
    assert_eq!(
        std::fs::read(file_path(workspace.path(), &a.id, batch, "a.txt")).unwrap(),
        b"first"
    );
    let early = UploadReceipt {
        session_id: a.id.clone(),
        batch_id: batch.clone(),
        token: allocation.files[0].token.clone(),
    };
    assert!(controller.uploaded_content(&a.id, &[early]).await.is_err());
    assert!(controller.catalog.try_lock().is_ok());
    drop(release);
    let uploaded = upload.await.unwrap().unwrap();
    *controller.upload_commit_gate.lock().unwrap() = None;
    assert_eq!(
        uploaded
            .iter()
            .map(|f| f.file.name.as_str())
            .collect::<Vec<_>>(),
        ["a.txt", "b.png"]
    );
    for f in &uploaded {
        assert_eq!(
            Path::new(&f.path),
            file_path(workspace.path(), &a.id, &f.file.batch_id, &f.file.name)
        );
    }
    assert!(
        controller
            .uploaded_content(&b.id, &[uploaded[0].receipt.clone()])
            .await
            .is_err()
    );
    let access = controller.acquire_session(&a.id, None).await.unwrap();
    let store = SqliteConversationStore::open_existing(
        a.active_conversation_id.clone(),
        &access.database_path,
    )
    .unwrap();
    assert!(
        store.load_canonical().unwrap().is_empty(),
        "upload/failed receipt admission creates no User message"
    );
    drop(store);
    drop(access);
    let (one, two) = tokio::join!(
        controller.upload(&a.id, None, vec![file("a.txt", b"one")]),
        controller.upload(&a.id, None, vec![file("a.txt", b"two")])
    );
    let one = one.unwrap();
    let two = two.unwrap();
    assert_ne!(one[0].path, two[0].path);
    drop(controller);
    let controller = SessionController::open(root.path()).unwrap();
    assert_eq!(
        controller
            .upload_status(&a.id, allocation.operation_id.as_ref().unwrap())
            .await
            .unwrap(),
        UploadOutcome::Ready {
            files: uploaded.clone()
        }
    );
    let next = controller
        .upload(&a.id, None, vec![file("a.txt", b"restart")])
        .await
        .unwrap();
    assert_ne!(next[0].path, one[0].path);
    assert!(
        controller
            .uploaded_content(&a.id, &[uploaded[0].receipt.clone()])
            .await
            .is_ok()
    );
    assert_eq!(std::fs::read(&uploaded[0].path).unwrap(), b"first");
    delete(&controller, &a.id).await;
    assert!(!Path::new(&uploaded[0].path).exists());
    assert!(workspace.path().is_dir());
    assert!(controller.read_session(&b.id).await.is_ok());
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fork_and_clone_copy_current_cut_before_publication_and_survive_source_delete() {
    for fork in [false, true] {
        let root = tempdir().unwrap();
        let workspace = tempdir().unwrap();
        let controller = SessionController::open(root.path()).unwrap();
        let source = controller
            .create_session(settings(workspace.path()))
            .await
            .unwrap()
            .session;
        let uploaded = controller
            .upload(&source.id, None, vec![file("report.txt", b"initial")])
            .await
            .unwrap();
        let orphan = controller
            .upload(&source.id, None, vec![file("not-in-cut.txt", b"orphan")])
            .await
            .unwrap();
        let access = controller.acquire_session(&source.id, None).await.unwrap();
        let store = SqliteConversationStore::open(
            source.active_conversation_id.clone(),
            &access.database_path,
        )
        .unwrap();
        store.append_canonical(&user("first", &uploaded)).unwrap();
        let revision = store.load_head().unwrap().revision;
        store.append_canonical(&user("later", &orphan)).unwrap();
        let full_revision = store.load_head().unwrap().revision;
        drop(store);
        drop(access);
        std::fs::write(&uploaded[0].path, b"current mutable bytes").unwrap();
        let gate = Arc::new(Gate::default());
        let release = gate.arm_scoped();
        *controller.copy_upload_gate.lock().unwrap() = Some(gate.clone());
        let worker = controller.clone();
        let id = source.id.clone();
        let copy = tokio::spawn(async move {
            if fork {
                worker
                    .fork_session(&id, None, full_revision, Some(&MessageId::new("later")))
                    .await
            } else {
                worker.clone_session(&id, None, revision).await
            }
        });
        tokio::task::spawn_blocking(move || gate.wait_entered())
            .await
            .unwrap();
        assert!(!copy.is_finished());
        assert_eq!(
            controller
                .list_sessions(None, 0, 20)
                .await
                .unwrap()
                .sessions
                .len(),
            1
        );
        let pending = pending_destination(&controller).await;
        let destination_path = file_path(
            workspace.path(),
            &pending,
            &uploaded[0].file.batch_id,
            &uploaded[0].file.name,
        );
        assert_eq!(
            std::fs::read(&destination_path).unwrap(),
            b"current mutable bytes"
        );
        assert_eq!(
            file_path(
                workspace.path(),
                &pending,
                &orphan[0].file.batch_id,
                &orphan[0].file.name
            )
            .exists(),
            fork
        );
        assert!(controller.catalog.try_lock().is_ok());
        drop(release);
        let destination = copy.await.unwrap().unwrap().session;
        *controller.copy_upload_gate.lock().unwrap() = None;
        delete(&controller, &source.id).await;
        assert_eq!(
            std::fs::read(&destination_path).unwrap(),
            b"current mutable bytes"
        );
        let access = controller
            .acquire_session(&destination.id, None)
            .await
            .unwrap();
        let store = SqliteConversationStore::open_existing(
            destination.active_conversation_id.clone(),
            &access.database_path,
        )
        .unwrap();
        let messages = crate::model::input::canonical_input(&store.load_canonical().unwrap());
        let projection =
            SessionUploadResolver::new(root.path(), destination.active_conversation_id.clone())
                .resolve(&messages)
                .unwrap();
        assert_eq!(projection.files[0].path, destination_path.to_str().unwrap());
        drop(store);
        drop(access);
        std::fs::remove_file(destination_path).unwrap();
        assert!(
            controller
                .clone_session(&destination.id, None, revision)
                .await
                .is_err()
        );
        assert_eq!(
            controller
                .list_sessions(None, 0, 20)
                .await
                .unwrap()
                .sessions
                .len(),
            1
        );
    }
}
#[tokio::test]
async fn deletion_freezes_all_historical_workspaces_and_retries_same_record() {
    let root = tempdir().unwrap();
    let first = tempdir().unwrap();
    let second = tempdir().unwrap();
    let controller = SessionController::open(root.path()).unwrap();
    let session = controller
        .create_session(settings(first.path()))
        .await
        .unwrap()
        .session;
    let other = controller
        .create_session(settings(first.path()))
        .await
        .unwrap()
        .session;
    let old = controller
        .upload(&session.id, None, vec![file("old", b"old")])
        .await
        .unwrap();
    let keep = controller
        .upload(&other.id, None, vec![file("keep", b"keep")])
        .await
        .unwrap();
    controller
        .catalog
        .lock()
        .await
        .replace_settings(&session.id, 0, settings(second.path()))
        .unwrap();
    let new = controller
        .upload(&session.id, None, vec![file("new", b"new")])
        .await
        .unwrap();
    let SessionDeleteResult::Preview { preview } = controller.delete_preview(&session.id).await
    else {
        panic!("preview");
    };
    assert_eq!(preview.upload_workspaces.len(), 2);
    let owned = first
        .path()
        .join(".agents/uploads")
        .join(session.id.as_str());
    let moved = first.path().join("held");
    std::fs::rename(&owned, &moved).unwrap();
    std::os::unix::fs::symlink(&moved, &owned).unwrap();
    let result = controller
        .delete_session(&session.id, &preview.target_revision)
        .await
        .unwrap();
    let SessionDeleteResult::CommittedCleanupPending { record, .. } = result else {
        panic!("must fail closed");
    };
    assert_eq!(record.upload_workspaces, preview.upload_workspaces);
    assert_eq!(std::fs::read(&keep[0].path).unwrap(), b"keep");
    std::fs::remove_file(&owned).unwrap();
    std::fs::rename(&moved, &owned).unwrap();
    drop(controller);
    let controller = SessionController::open(root.path()).unwrap();
    assert!(matches!(
        controller.recover_deletion(&session.id).await,
        SessionDeleteResult::Deleted { .. }
    ));
    assert!(!Path::new(&old[0].path).exists());
    assert!(!Path::new(&new[0].path).exists());
    assert_eq!(std::fs::read(&keep[0].path).unwrap(), b"keep");
}

#[test]
fn exclusive_batch_creation_never_overwrites_and_cleanup_never_follows_child_links() {
    let workspace = tempdir().unwrap();
    let outside = tempdir().unwrap();
    let session = SessionId::new("ses_84097828-fc31-78c8-8292-10df48901a85");
    let mut registry = UploadRegistry::default();
    let files = vec![file("safe", b"original")];
    let batch = registry
        .claim(workspace.path().canonicalize().unwrap(), &files)
        .unwrap();
    registry.materialize(&session, &batch, &files).unwrap();
    assert!(
        registry
            .materialize(&session, &batch, &[file("safe", b"replacement")])
            .is_err()
    );
    assert_eq!(
        std::fs::read(file_path(workspace.path(), &session, &batch, "safe")).unwrap(),
        b"original"
    );
    std::fs::write(outside.path().join("keep"), b"outside").unwrap();
    std::os::unix::fs::symlink(
        outside.path(),
        file_path(workspace.path(), &session, &batch, "link"),
    )
    .unwrap();
    cleanup(workspace.path(), &session).unwrap();
    cleanup(workspace.path(), &session).unwrap();
    assert_eq!(
        std::fs::read(outside.path().join("keep")).unwrap(),
        b"outside"
    );
}

#[tokio::test]
async fn same_session_branch_shares_uploads_and_private_copy_claim_recovers_after_restart() {
    let root = tempdir().unwrap();
    let workspace = tempdir().unwrap();
    let controller = SessionController::open(root.path()).unwrap();
    let session = controller
        .create_session(settings(workspace.path()))
        .await
        .unwrap()
        .session;
    let uploaded = controller
        .upload(&session.id, None, vec![file("source", b"bytes")])
        .await
        .unwrap();
    let access = controller.acquire_session(&session.id, None).await.unwrap();
    let store = SqliteConversationStore::open(
        session.active_conversation_id.clone(),
        &access.database_path,
    )
    .unwrap();
    store.append_canonical(&user("first", &uploaded)).unwrap();
    store.append_canonical(&user("boundary", &[])).unwrap();
    let revision = store.load_head().unwrap().revision;
    drop(store);
    drop(access);
    let before = controller
        .catalog
        .lock()
        .await
        .upload_registry(&session.id)
        .unwrap();
    let branch = controller
        .branch_session_node(
            &session.id,
            &session.active_node,
            revision,
            &MessageId::new("boundary"),
        )
        .await
        .unwrap();
    assert_eq!(branch.session.id, session.id);
    assert_eq!(
        controller
            .catalog
            .lock()
            .await
            .upload_registry(&session.id)
            .unwrap(),
        before
    );
    assert_eq!(
        std::fs::read_dir(workspace.path().join(".agents/uploads"))
            .unwrap()
            .count(),
        1
    );
    let staged = SessionId::new("ses_5d906140-8048-712d-8539-25aed45333a1");
    controller
        .catalog
        .lock()
        .await
        .claim_upload_preparation(&staged, &[workspace.path().to_path_buf()])
        .unwrap();
    let staged_root = session_directory(workspace.path(), &staged, true).unwrap();
    drop(staged_root);
    drop(controller);
    let controller = SessionController::open(root.path()).unwrap();
    assert!(
        !workspace
            .path()
            .join(".agents/uploads")
            .join(staged.as_str())
            .exists()
    );
    assert!(
        controller
            .catalog
            .lock()
            .await
            .document
            .upload_preparations
            .is_empty()
    );
    assert_eq!(std::fs::read(&uploaded[0].path).unwrap(), b"bytes");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ancestor_swap_before_readiness_fails_closed_and_remains_owned() {
    let root = tempdir().unwrap();
    let workspace = tempdir().unwrap();
    let outside = tempdir().unwrap();
    let controller = SessionController::open(root.path()).unwrap();
    let session = controller
        .create_session(settings(workspace.path()))
        .await
        .unwrap()
        .session;
    let gate = Arc::new(Gate::default());
    let release = gate.arm_scoped();
    *controller.upload_commit_gate.lock().unwrap() = Some(gate.clone());
    let worker = controller.clone();
    let id = session.id.clone();
    let upload = tokio::spawn(async move {
        worker
            .upload(&id, None, vec![file("race.txt", b"owned")])
            .await
    });
    tokio::task::spawn_blocking(move || gate.wait_entered())
        .await
        .unwrap();
    let path = workspace
        .path()
        .join(".agents/uploads")
        .join(session.id.as_str());
    let parked = workspace.path().join("parked");
    std::fs::rename(&path, &parked).unwrap();
    std::os::unix::fs::symlink(outside.path(), &path).unwrap();
    drop(release);
    assert!(upload.await.unwrap().is_err());
    assert_eq!(std::fs::read_dir(outside.path()).unwrap().count(), 0);
    let registry = controller
        .catalog
        .lock()
        .await
        .upload_registry(&session.id)
        .unwrap();
    assert_eq!(registry.allocations.len(), 1);
    assert!(registry.allocations.values().all(|a| !a.ready));
    std::fs::remove_file(&path).unwrap();
    std::fs::rename(&parked, &path).unwrap();
    delete(&controller, &session.id).await;
    assert!(!path.exists());
}

#[tokio::test]
async fn context_estimates_the_same_rendering_as_provider_input() {
    use crate::context::{ContextConfig, ContextEngine, DefaultTokenEstimator, TokenEstimator};
    let root = tempdir().unwrap();
    let workspace = tempdir().unwrap();
    let controller = SessionController::open(root.path()).unwrap();
    let session = controller
        .create_session(settings(workspace.path()))
        .await
        .unwrap()
        .session;
    let files = controller
        .upload(
            &session.id,
            None,
            vec![file("tokens.txt", b"no eager bytes")],
        )
        .await
        .unwrap();
    let canonical = crate::model::input::canonical_input(&[user("input", &files)]);
    let estimator = Arc::new(DefaultTokenEstimator);
    let mut engine = ContextEngine::new(
        ContextConfig {
            context_window_tokens: 100_000,
            reserve_tokens: 0,
            keep_recent_tokens: 1,
        },
        estimator.clone(),
    )
    .unwrap();
    engine.set_upload_resolver(Arc::new(SessionUploadResolver::new(
        root.path(),
        session.active_conversation_id,
    )));
    let mut rendered = canonical.clone();
    engine.project_uploads(&mut rendered).unwrap();
    assert_eq!(
        engine.estimate_model_input(&canonical, "system", &[]),
        estimator.estimate_input(&rendered, "system", &[])
    );
    assert!(
        engine.estimate_model_input(&canonical, "system", &[])
            > estimator.estimate_input(&canonical, "system", &[])
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn copied_file_symlink_before_publication_rejects_and_cleans_private_destination() {
    let root = tempdir().unwrap();
    let workspace = tempdir().unwrap();
    let outside = tempdir().unwrap();
    let controller = SessionController::open(root.path()).unwrap();
    let source = controller
        .create_session(settings(workspace.path()))
        .await
        .unwrap()
        .session;
    let files = controller
        .upload(&source.id, None, vec![file("copy.txt", b"source")])
        .await
        .unwrap();
    let access = controller.acquire_session(&source.id, None).await.unwrap();
    let store =
        SqliteConversationStore::open(source.active_conversation_id.clone(), &access.database_path)
            .unwrap();
    store.append_canonical(&user("input", &files)).unwrap();
    let revision = store.load_head().unwrap().revision;
    drop(store);
    drop(access);
    let gate = Arc::new(Gate::default());
    let release = gate.arm_scoped();
    *controller.copy_upload_gate.lock().unwrap() = Some(gate.clone());
    let worker = controller.clone();
    let id = source.id.clone();
    let copy = tokio::spawn(async move { worker.clone_session(&id, None, revision).await });
    tokio::task::spawn_blocking(move || gate.wait_entered())
        .await
        .unwrap();
    let destination = pending_destination(&controller).await;
    let copied = file_path(
        workspace.path(),
        &destination,
        &files[0].file.batch_id,
        "copy.txt",
    );
    let keep = outside.path().join("keep");
    std::fs::write(&keep, b"untouched").unwrap();
    std::fs::remove_file(&copied).unwrap();
    std::os::unix::fs::symlink(&keep, &copied).unwrap();
    drop(release);
    assert!(copy.await.unwrap().is_err());
    assert_eq!(
        controller
            .list_sessions(None, 0, 20)
            .await
            .unwrap()
            .sessions
            .len(),
        1
    );
    assert!(
        !workspace
            .path()
            .join(".agents/uploads")
            .join(destination.as_str())
            .exists()
    );
    assert!(
        !root
            .path()
            .join("sessions")
            .join(destination.as_str())
            .exists()
    );
    assert!(
        controller
            .catalog
            .lock()
            .await
            .document
            .upload_preparations
            .is_empty()
    );
    assert_eq!(std::fs::read(&keep).unwrap(), b"untouched");
    assert_eq!(std::fs::read(&files[0].path).unwrap(), b"source");
}

#[tokio::test]
async fn uploads_cannot_nest_inside_another_sessions_cleanup_root() {
    let root = tempdir().unwrap();
    let workspace = tempdir().unwrap();
    let controller = SessionController::open(root.path()).unwrap();
    let a = controller
        .create_session(settings(workspace.path()))
        .await
        .unwrap()
        .session;
    let owned = controller
        .upload(&a.id, None, vec![file("outer", b"owned")])
        .await
        .unwrap();
    let nested_workspace = Path::new(&owned[0].path).parent().unwrap();
    let b = controller
        .create_session(settings(nested_workspace))
        .await
        .unwrap()
        .session;
    assert!(
        controller
            .upload(&b.id, None, vec![file("nested", b"forbidden")])
            .await
            .is_err()
    );
    assert!(
        controller
            .catalog
            .lock()
            .await
            .upload_registry(&b.id)
            .unwrap()
            .allocations
            .is_empty()
    );
    delete(&controller, &a.id).await;
    assert!(controller.read_session(&b.id).await.is_ok());
    delete(&controller, &b.id).await;
    assert!(workspace.path().is_dir());
}

#[tokio::test]
async fn failed_copy_cleanup_retains_the_frozen_claim_until_recovery_finishes() {
    let root = tempdir().unwrap();
    let workspace = tempdir().unwrap();
    let controller = SessionController::open(root.path()).unwrap();
    let source = controller
        .create_session(settings(workspace.path()))
        .await
        .unwrap()
        .session;
    let files = controller
        .upload(
            &source.id,
            None,
            vec![file("first", b"source"), file("missing", b"remove")],
        )
        .await
        .unwrap();
    let access = controller.acquire_session(&source.id, None).await.unwrap();
    let store =
        SqliteConversationStore::open(source.active_conversation_id.clone(), &access.database_path)
            .unwrap();
    store.append_canonical(&user("input", &files)).unwrap();
    let revision = store.load_head().unwrap().revision;
    drop(store);
    drop(access);
    std::fs::remove_file(&files[1].path).unwrap();
    controller
        .catalog
        .lock()
        .await
        .preparation_cleanup_fault
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let error = controller
        .clone_session(&source.id, None, revision)
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        SessionError::PreparationCleanupPending { .. }
    ));
    let destination = pending_destination(&controller).await;
    let residue = file_path(
        workspace.path(),
        &destination,
        &files[0].file.batch_id,
        "first",
    );
    assert_eq!(std::fs::read(&residue).unwrap(), b"source");
    assert!(
        root.path()
            .join("sessions")
            .join(destination.as_str())
            .is_dir()
    );
    assert!(controller.read_session(&destination).await.is_err());
    let frozen = controller
        .catalog
        .lock()
        .await
        .document
        .upload_preparations
        .clone();
    assert_eq!(frozen[&destination], vec![workspace.path().to_path_buf()]);
    assert!(
        controller
            .catalog
            .lock()
            .await
            .recover_upload_preparations()
            .is_err()
    );
    assert_eq!(
        controller.catalog.lock().await.document.upload_preparations,
        frozen
    );
    // The claim reserves identity even if native residue was independently removed.
    // A new Session must not consume this still-owned upload cleanup authority.
    std::fs::remove_dir_all(root.path().join("sessions").join(destination.as_str())).unwrap();
    let unrelated = controller
        .create_session(settings(workspace.path()))
        .await
        .unwrap()
        .session;
    assert_ne!(unrelated.id, destination);
    assert_eq!(
        controller.catalog.lock().await.document.upload_preparations,
        frozen
    );
    // The fault is removed at process restart; the durable workset must survive it.
    drop(controller);
    let controller = SessionController::open(root.path()).unwrap();
    assert!(
        controller
            .catalog
            .lock()
            .await
            .document
            .upload_preparations
            .is_empty()
    );
    assert!(!residue.exists());
    assert!(controller.read_session(&unrelated.id).await.is_ok());
    assert!(
        !root
            .path()
            .join("sessions")
            .join(destination.as_str())
            .exists()
    );
    assert_eq!(std::fs::read(&files[0].path).unwrap(), b"source");
    assert!(controller.read_session(&source.id).await.is_ok());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn branch_publication_faults_preserve_commit_semantics_and_only_discard_private_nodes() {
    for after_rename in [false, true] {
        let root = tempdir().unwrap();
        let workspace = tempdir().unwrap();
        let controller = SessionController::open(root.path()).unwrap();
        let source = controller
            .create_session(settings(workspace.path()))
            .await
            .unwrap()
            .session;
        let other = controller
            .create_session(settings(workspace.path()))
            .await
            .unwrap()
            .session;
        let source_files = controller
            .upload(&source.id, None, vec![file("source", b"source")])
            .await
            .unwrap();
        let other_files = controller
            .upload(&other.id, None, vec![file("other", b"other")])
            .await
            .unwrap();
        let access = controller.acquire_session(&source.id, None).await.unwrap();
        let store = SqliteConversationStore::open(
            source.active_conversation_id.clone(),
            &access.database_path,
        )
        .unwrap();
        store
            .append_canonical(&user("first", &source_files))
            .unwrap();
        store.append_canonical(&user("boundary", &[])).unwrap();
        let revision = store.load_head().unwrap().revision;
        let conversations = access
            .database_path
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf();
        drop(store);
        drop(access);
        let before = controller.read_session(&source.id).await.unwrap();
        let existing = std::fs::read_dir(&conversations)
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect::<Vec<_>>();
        let gate = Arc::new(Gate::default());
        let release = gate.arm_scoped();
        *controller.copy_publication_gate.lock().unwrap() = Some(gate.clone());
        let worker = controller.clone();
        let id = source.id.clone();
        let node = source.active_node.clone();
        let branch = tokio::spawn(async move {
            worker
                .branch_session_node(&id, &node, revision, &MessageId::new("boundary"))
                .await
        });
        tokio::task::spawn_blocking(move || gate.wait_entered())
            .await
            .unwrap();
        let staged = std::fs::read_dir(&conversations)
            .unwrap()
            .map(|e| e.unwrap().path())
            .find(|p| !existing.contains(p))
            .unwrap();
        assert!(controller.catalog.try_lock().is_ok());
        if after_rename {
            controller
                .catalog
                .lock()
                .await
                .arm_write_fault_after_rename();
        } else {
            controller
                .catalog
                .lock()
                .await
                .arm_write_fault_before_rename();
        }
        drop(release);
        let result = branch.await.unwrap();
        if after_rename {
            let result = result.unwrap();
            assert!(result.durability_diagnostic.is_some());
            assert_eq!(result.session.node_count, 2);
            assert!(staged.exists());
        } else {
            assert!(matches!(
                result,
                Err(SessionError::CatalogCommit {
                    error: super::super::CatalogCommitError::NotCommitted { .. }
                })
            ));
            assert_eq!(controller.read_session(&source.id).await.unwrap(), before);
            assert!(!staged.exists());
            assert_eq!(
                std::fs::read_dir(&conversations).unwrap().count(),
                existing.len()
            );
        }
        assert_eq!(std::fs::read(&source_files[0].path).unwrap(), b"source");
        assert_eq!(std::fs::read(&other_files[0].path).unwrap(), b"other");
        assert!(controller.read_session(&other.id).await.is_ok());
        assert!(
            controller
                .catalog
                .lock()
                .await
                .document
                .upload_preparations
                .is_empty()
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn restored_editor_uploads_are_ordered_owned_and_prepared_before_publication() {
    for tree in [false, true] {
        let root = tempdir().unwrap();
        let workspace = tempdir().unwrap();
        let controller = SessionController::open(root.path()).unwrap();
        let source = controller
            .create_session(settings(workspace.path()))
            .await
            .unwrap()
            .session;
        let files = controller
            .upload(&source.id, None, vec![file("A", b"A"), file("B", b"B")])
            .await
            .unwrap();
        let body = "\n Please analyze it.  \r\n";
        let content = vec![
            UserContentBlock::UploadedFile(files[0].file.clone()),
            UserContentBlock::Text(crate::message::content::TextBlock { text: body.into() }),
            UserContentBlock::UploadedFile(files[1].file.clone()),
        ];
        let access = controller.acquire_session(&source.id, None).await.unwrap();
        let store = SqliteConversationStore::open(
            source.active_conversation_id.clone(),
            &access.database_path,
        )
        .unwrap();
        let MessageBlock::User(mut input) = user("boundary", &[]) else {
            unreachable!()
        };
        input.content = content.clone();
        store.append_canonical(&MessageBlock::User(input)).unwrap();
        let revision = store.load_head().unwrap().revision;
        drop(store);
        drop(access);
        let gate = Arc::new(Gate::default());
        let release = gate.arm_scoped();
        *controller.copy_publication_gate.lock().unwrap() = Some(gate.clone());
        let worker = controller.clone();
        let id = source.id.clone();
        let node = source.active_node.clone();
        let fork = tokio::spawn(async move {
            worker
                .copy_lineage(
                    &id,
                    Some(&node),
                    revision,
                    Some(&MessageId::new("boundary")),
                    tree,
                    crate::local_runtime::session::LineageSide::Before,
                )
                .await
        });
        tokio::task::spawn_blocking(move || gate.wait_entered())
            .await
            .unwrap();
        let destination = if tree {
            source.id.clone()
        } else {
            pending_destination(&controller).await
        };
        for f in &files {
            assert!(
                file_path(
                    workspace.path(),
                    &destination,
                    &f.file.batch_id,
                    &f.file.name
                )
                .exists()
            );
        }
        assert_eq!(
            controller
                .list_sessions(None, 0, 20)
                .await
                .unwrap()
                .sessions
                .len(),
            1
        );
        drop(release);
        let result = fork.await.unwrap().unwrap();
        let editor = result.editor_content.unwrap();
        let [
            UserInputBlock::Upload(a),
            UserInputBlock::Text(text),
            UserInputBlock::Upload(b),
        ] = &editor[..]
        else {
            panic!("exact restored order");
        };
        assert_eq!(text.text, body);
        assert_eq!(a.session_id, destination);
        assert_eq!(b.session_id, destination);
        if !tree {
            assert!(
                controller
                    .uploaded_content(&destination, &[files[0].receipt.clone()])
                    .await
                    .is_err()
            );
            delete(&controller, &source.id).await;
        }
        assert_eq!(
            controller
                .uploaded_content(&destination, &[a.clone(), b.clone()])
                .await
                .unwrap(),
            vec![content[0].clone(), content[2].clone()]
        );
        for f in &files {
            assert_eq!(
                std::fs::read(file_path(
                    workspace.path(),
                    &destination,
                    &f.file.batch_id,
                    &f.file.name
                ))
                .unwrap(),
                f.file.name.as_bytes()
            );
        }
        assert_eq!(
            std::fs::read_dir(workspace.path().join(".agents/uploads"))
                .unwrap()
                .count(),
            1
        );
    }
}

/// Failed materialization keeps the durable not-ready claim (deletion work)
/// and never yields a usable receipt, even though bytes are already visible.
#[tokio::test]
async fn native_sync_failures_never_commit_ready() {
    for stage in ["file sync", "directory sync"] {
        let root = tempdir().unwrap();
        let workspace = tempdir().unwrap();
        let controller = SessionController::open(root.path()).unwrap();
        let id = controller
            .create_session(settings(workspace.path()))
            .await
            .unwrap()
            .session
            .id;
        arm_checkpoint(stage, || {
            Err(io::Error::other("injected native sync failure"))
        });
        let error = controller
            .upload(&id, None, vec![file("payload.txt", &[42; 4096])])
            .await
            .unwrap_err();
        assert!(checkpoint_consumed(), "{stage} was never reached");
        assert!(
            matches!(&error, SessionError::Catalog { detail } if detail == "injected native sync failure"),
            "{stage}: {error}"
        );
        drop(controller);
        let controller = SessionController::open(root.path()).unwrap();
        let registry = controller
            .catalog
            .lock()
            .await
            .upload_registry(&id)
            .unwrap();
        let [(batch, allocation)] = registry.allocations.iter().collect::<Vec<_>>()[..] else {
            panic!("{stage}: exactly one durable claim");
        };
        assert!(!allocation.ready, "{stage}");
        assert_eq!(
            controller
                .upload_status(&id, allocation.operation_id.as_ref().unwrap())
                .await
                .unwrap(),
            UploadOutcome::Failed
        );
        assert_eq!(allocation.workspace, workspace.path());
        assert_eq!(allocation.files[0].name, "payload.txt");
        let path = file_path(workspace.path(), &id, batch, "payload.txt");
        assert_eq!(std::fs::read(&path).unwrap(), [42_u8; 4096]);
        assert!(registry.receipts(&id, batch).is_err(), "{stage}");
        let receipt = UploadReceipt {
            session_id: id.clone(),
            batch_id: batch.clone(),
            token: allocation.files[0].token.clone(),
        };
        assert!(controller.uploaded_content(&id, &[receipt]).await.is_err());
        delete(&controller, &id).await;
        assert!(!path.exists(), "{stage}: the claim is deletion work");
    }
}

/// Complete synced bytes still produce no receipt when the ready commit fails.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ready_registry_commit_failure_never_issues_a_receipt() {
    let root = tempdir().unwrap();
    let workspace = tempdir().unwrap();
    let controller = SessionController::open(root.path()).unwrap();
    let id = controller
        .create_session(settings(workspace.path()))
        .await
        .unwrap()
        .session
        .id;
    let gate = Arc::new(Gate::default());
    let release = gate.arm_scoped();
    *controller.upload_commit_gate.lock().unwrap() = Some(gate.clone());
    let worker = controller.clone();
    let upload_id = id.clone();
    let upload = tokio::spawn(async move {
        worker
            .upload(&upload_id, None, vec![file("synced.txt", b"complete")])
            .await
    });
    tokio::task::spawn_blocking(move || gate.wait_entered())
        .await
        .unwrap();
    controller
        .catalog
        .lock()
        .await
        .arm_write_fault_before_rename();
    drop(release);
    assert!(matches!(
        upload.await.unwrap(),
        Err(SessionError::CatalogCommit {
            error: super::super::CatalogCommitError::NotCommitted { .. }
        })
    ));
    *controller.upload_commit_gate.lock().unwrap() = None;
    let registry = controller
        .catalog
        .lock()
        .await
        .upload_registry(&id)
        .unwrap();
    let [(batch, allocation)] = registry.allocations.iter().collect::<Vec<_>>()[..] else {
        panic!("exactly one durable claim");
    };
    assert!(!allocation.ready);
    assert!(allocation.failed);
    assert_eq!(
        controller
            .upload_status(&id, allocation.operation_id.as_ref().unwrap())
            .await
            .unwrap(),
        UploadOutcome::Failed
    );
    assert_eq!(
        std::fs::read(file_path(workspace.path(), &id, batch, "synced.txt")).unwrap(),
        b"complete"
    );
    assert!(registry.receipts(&id, batch).is_err());
    let receipt = UploadReceipt {
        session_id: id.clone(),
        batch_id: batch.clone(),
        token: allocation.files[0].token.clone(),
    };
    assert!(controller.uploaded_content(&id, &[receipt]).await.is_err());
}

/// A declared component is substituted at the `directory sync` checkpoint:
/// after every file was written and synced through retained descriptors,
/// before the directory sync barriers and the readiness re-open. The synced
/// bytes stay with the moved object, and the re-open's identity check refuses
/// to publish the substituted tree. This proves nothing about a substitution
/// before or between the file writes.
#[tokio::test]
async fn declared_path_substitution_during_materialization_never_commits_ready() {
    for component in ["workspace", ".agents", "uploads", "session", "batch"] {
        let root = tempdir().unwrap();
        let holder = tempdir().unwrap();
        let workspace = holder.path().join("workspace");
        std::fs::create_dir(&workspace).unwrap();
        let controller = SessionController::open(root.path()).unwrap();
        let id = controller
            .create_session(settings(&workspace))
            .await
            .unwrap()
            .session
            .id;
        let owned = workspace.join(".agents/uploads").join(id.as_str());
        let parked = holder.path().join("parked");
        let (declared, target, base) = (owned.clone(), parked.clone(), workspace.clone());
        arm_checkpoint("directory sync", move || {
            let batch = std::fs::read_dir(&declared)?.next().unwrap()?.file_name();
            let moved = match component {
                "workspace" => base,
                ".agents" => base.join(".agents"),
                "uploads" => base.join(".agents/uploads"),
                "session" => declared.clone(),
                _ => declared.join(&batch),
            };
            std::fs::rename(moved, target)?;
            std::fs::create_dir_all(declared.join(batch))
        });
        let error = controller
            .upload(&id, None, vec![file("payload.txt", b"retained")])
            .await
            .unwrap_err();
        assert!(checkpoint_consumed(), "{component}");
        assert!(
            matches!(&error, SessionError::Catalog { detail }
                if detail == "upload path changed during materialization"),
            "{component}: {error}"
        );
        let registry = controller
            .catalog
            .lock()
            .await
            .upload_registry(&id)
            .unwrap();
        let [(batch, allocation)] = registry.allocations.iter().collect::<Vec<_>>()[..] else {
            panic!("{component}: exactly one durable claim");
        };
        assert!(!allocation.ready, "{component}");
        assert_eq!(
            controller
                .upload_status(&id, allocation.operation_id.as_ref().unwrap())
                .await
                .unwrap(),
            UploadOutcome::Failed
        );
        assert!(registry.receipts(&id, batch).is_err(), "{component}");
        let suffix = match component {
            "workspace" => Path::new(".agents/uploads").join(id.as_str()).join(batch),
            ".agents" => Path::new("uploads").join(id.as_str()).join(batch),
            "uploads" => Path::new(id.as_str()).join(batch),
            "session" => PathBuf::from(batch),
            _ => PathBuf::new(),
        };
        assert_eq!(
            std::fs::read(parked.join(suffix).join("payload.txt")).unwrap(),
            b"retained"
        );
        assert_eq!(
            std::fs::read_dir(owned.join(batch)).unwrap().count(),
            0,
            "{component}: the substituted tree is never adopted"
        );
    }
}

#[test]
fn workspace_policy_and_missing_upload_inspection_create_nothing() {
    let root = tempdir().unwrap();
    assert!(validate_workspace(Path::new("relative")).is_err());
    assert!(validate_workspace(&root.path().join("../bad")).is_err());
    assert!(
        session_directory(
            root.path(),
            &SessionId::new("ses_84097828-fc31-78c8-8292-10df48901a85"),
            false
        )
        .is_err()
    );
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
}

/// Native traversal rejects a symlink at any declared component, whatever it
/// points at; only the trusted boundary's canonical spelling is traversed.
#[test]
fn every_symlinked_declared_component_fails_closed() {
    for location in ["ancestor", "root", "intermediate", "leaf"] {
        for target in ["inside", "outside", "dangling"] {
            let root = tempdir().unwrap();
            let outside = tempdir().unwrap();
            let suffix = match location {
                "ancestor" => "workspace/.agents/uploads/session/batch",
                "root" => ".agents/uploads/session/batch",
                "intermediate" => "session/batch",
                _ => "",
            };
            std::fs::create_dir_all(root.path().join("real").join(suffix)).unwrap();
            std::fs::create_dir_all(outside.path().join(suffix)).unwrap();
            let destination = match target {
                "inside" => PathBuf::from("real"),
                "outside" => outside.path().to_path_buf(),
                _ => PathBuf::from("absent"),
            };
            symlink(destination, root.path().join("link")).unwrap();
            let declared = if suffix.is_empty() {
                root.path().join("link")
            } else {
                root.path().join("link").join(suffix)
            };
            assert!(stable_directory(&declared).is_err(), "{location}/{target}");
            match declared.canonicalize() {
                Ok(canonical) => assert!(stable_directory(&canonical).is_ok()),
                Err(_) => assert_eq!(target, "dangling", "{location}"),
            }
        }
    }
}

/// Acquired parent -> next component renamed and a link installed -> resume.
#[test]
fn component_replaced_after_parent_acquisition_fails_closed() {
    for target in ["inside", "outside", "dangling"] {
        let root = tempdir().unwrap();
        let outside = tempdir().unwrap();
        std::fs::create_dir(root.path().join("next")).unwrap();
        let parent = stable_directory(root.path()).unwrap();
        std::fs::rename(root.path().join("next"), root.path().join("parked")).unwrap();
        let destination = match target {
            "inside" => PathBuf::from("parked"),
            "outside" => outside.path().to_path_buf(),
            _ => PathBuf::from("missing"),
        };
        symlink(destination, root.path().join("next")).unwrap();
        assert!(
            directory_at(&parent, OsStr::new("next")).is_err(),
            "{target}"
        );
        assert_eq!(std::fs::read_dir(outside.path()).unwrap().count(), 0);
    }
}

#[test]
fn materialized_modes_and_leaf_types_are_verified() {
    let root = tempdir().unwrap();
    let outside = tempdir().unwrap();
    let id = SessionId::new("ses_84097828-fc31-78c8-8292-10df48901a85");
    let files = vec![file("payload.txt", &[42; 4096])];
    let mut registry = UploadRegistry::default();
    let batch = registry.claim(root.path().to_path_buf(), &files).unwrap();
    registry.materialize(&id, &batch, &files).unwrap();
    registry.verify_materialized(&id, &batch).unwrap();
    let leaf = file_path(root.path(), &id, &batch, "payload.txt");
    let directory = leaf.parent().unwrap();
    let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&leaf), 0o600);
    assert_eq!(mode(directory), 0o700);
    std::fs::write(directory.join("inside"), b"keep").unwrap();
    std::fs::write(outside.path().join("outside"), b"keep").unwrap();
    std::fs::remove_file(&leaf).unwrap();
    for target in [
        PathBuf::from("inside"),
        outside.path().join("outside"),
        PathBuf::from("dangling"),
    ] {
        symlink(&target, &leaf).unwrap();
        assert!(
            registry.verify_materialized(&id, &batch).is_err(),
            "{target:?}"
        );
        std::fs::remove_file(&leaf).unwrap();
    }
    // Nonblocking open: a FIFO without a writer is rejected, never waited on.
    nix::unistd::mkfifo(&leaf, Mode::S_IRUSR | Mode::S_IWUSR).unwrap();
    assert!(registry.verify_materialized(&id, &batch).is_err());
    std::fs::remove_file(&leaf).unwrap();
    std::fs::create_dir(&leaf).unwrap();
    assert!(registry.verify_materialized(&id, &batch).is_err());
    assert_eq!(std::fs::read(directory.join("inside")).unwrap(), b"keep");
    assert_eq!(
        std::fs::read(outside.path().join("outside")).unwrap(),
        b"keep"
    );
}

#[test]
fn native_policy_boundaries_are_independent_of_json_and_images() {
    let metadata = |sizes: &[usize]| {
        sizes
            .iter()
            .enumerate()
            .map(|(i, size)| UploadMetadata {
                name: format!("document-{i}.md"),
                size: *size,
            })
            .collect::<Vec<_>>()
    };
    for size in [
        UPLOAD_POLICY.max_file_bytes - 1,
        UPLOAD_POLICY.max_file_bytes,
    ] {
        assert!(validate_metadata(&metadata(&[size])).is_ok());
    }
    assert!(validate_metadata(&metadata(&[UPLOAD_POLICY.max_file_bytes + 1])).is_err());
    for total in [
        UPLOAD_POLICY.max_transfer_bytes - 1,
        UPLOAD_POLICY.max_transfer_bytes,
    ] {
        assert!(
            validate_metadata(&metadata(&[
                UPLOAD_POLICY.max_file_bytes,
                total - UPLOAD_POLICY.max_file_bytes
            ]))
            .is_ok()
        );
    }
    assert!(
        validate_metadata(&metadata(&[
            UPLOAD_POLICY.max_file_bytes,
            UPLOAD_POLICY.max_file_bytes,
            1
        ]))
        .is_err()
    );
    for count in [
        UPLOAD_POLICY.max_files_per_transfer - 1,
        UPLOAD_POLICY.max_files_per_transfer,
    ] {
        assert!(validate_metadata(&metadata(&vec![0; count])).is_ok());
    }
    for count in [0, UPLOAD_POLICY.max_files_per_transfer + 1] {
        assert!(validate_metadata(&metadata(&vec![0; count])).is_err());
    }
}

#[tokio::test]
async fn deletion_loses_to_native_upload_exclusion_at_each_materialization_boundary() {
    for stage in [
        "allocation claim",
        "batch directory",
        "file write",
        "file sync",
        "directory sync",
    ] {
        let root = tempdir().unwrap();
        let workspace = tempdir().unwrap();
        let controller = SessionController::open(root.path()).unwrap();
        let id = controller
            .create_session(settings(workspace.path()))
            .await
            .unwrap()
            .session
            .id;
        let (entered, observed) = tokio::sync::oneshot::channel();
        let (release, released) = std::sync::mpsc::sync_channel(0);
        arm_checkpoint(stage, move || {
            entered.send(()).unwrap();
            released.recv().unwrap();
            Ok(())
        });
        let operation = "a".repeat(32);
        let upload = controller.upload_correlated(
            &id,
            None,
            operation.clone(),
            vec![file("document.md", b"# durable document")],
        );
        let deletion = async {
            observed.await.unwrap();
            let registry = controller
                .catalog
                .lock()
                .await
                .upload_registry(&id)
                .unwrap();
            let (batch, _) = registry.allocations.iter().next().unwrap();
            assert!(registry.receipts(&id, batch).is_err(), "{stage}");
            assert_eq!(
                controller.upload_status(&id, &operation).await.unwrap(),
                UploadOutcome::Unresolved
            );
            let SessionDeleteResult::Preview { preview } = controller.delete_preview(&id).await
            else {
                panic!("read-only preview")
            };
            assert!(
                matches!(
                    controller
                        .delete_session(&id, &preview.target_revision)
                        .await
                        .unwrap(),
                    SessionDeleteResult::Blocked {
                        reason: super::super::deletion::DeletionBlocker::ResourceConflict,
                        ..
                    }
                ),
                "{stage}"
            );
            release.send(()).unwrap();
        };
        let (result, ()) = tokio::join!(upload, deletion);
        assert!(checkpoint_consumed(), "{stage} was not consumed");
        let files = result.unwrap();
        assert_eq!(
            controller.upload_status(&id, &operation).await.unwrap(),
            UploadOutcome::Ready { files }
        );
        delete(&controller, &id).await;
        assert!(
            controller
                .upload_correlated(&id, None, "b".repeat(32), vec![file("late.md", b"late")])
                .await
                .is_err()
        );
        assert!(
            !workspace
                .path()
                .join(".agents/uploads")
                .join(id.as_str())
                .exists()
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn post_materialization_verification_failure_is_durable_failed_and_retryable() {
    let root = tempdir().unwrap();
    let workspace = tempdir().unwrap();
    let controller = SessionController::open(root.path()).unwrap();
    let id = controller
        .create_session(settings(workspace.path()))
        .await
        .unwrap()
        .session
        .id;
    let gate = Arc::new(Gate::default());
    let release = gate.arm_scoped();
    *controller.upload_commit_gate.lock().unwrap() = Some(gate.clone());
    let worker = controller.clone();
    let addressed = id.clone();
    let operation = "b".repeat(32);
    let correlation = operation.clone();
    let upload = tokio::spawn(async move {
        worker
            .upload_correlated(
                &addressed,
                None,
                correlation,
                vec![file("verified.txt", b"synced")],
            )
            .await
    });
    tokio::task::spawn_blocking(move || gate.wait_entered())
        .await
        .unwrap();
    let registry = controller
        .catalog
        .lock()
        .await
        .upload_registry(&id)
        .unwrap();
    let batch = registry.allocations.keys().next().unwrap();
    let path = file_path(workspace.path(), &id, batch, "verified.txt");
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    drop(release);
    assert!(upload.await.unwrap().is_err());
    *controller.upload_commit_gate.lock().unwrap() = None;
    assert_eq!(
        controller.upload_status(&id, &operation).await.unwrap(),
        UploadOutcome::Failed
    );
    let registry = controller
        .catalog
        .lock()
        .await
        .upload_registry(&id)
        .unwrap();
    assert!(registry.receipts(&id, batch).is_err());
    assert!(registry.allocations[batch].failed);
    controller
        .upload_correlated(
            &id,
            None,
            "c".repeat(32),
            vec![file("retry.txt", b"explicit retry")],
        )
        .await
        .unwrap();
    assert_eq!(
        controller
            .catalog
            .lock()
            .await
            .upload_registry(&id)
            .unwrap()
            .allocations
            .len(),
        2
    );
    drop(controller);
    let reopened = SessionController::open(root.path()).unwrap();
    assert_eq!(
        reopened.upload_status(&id, &operation).await.unwrap(),
        UploadOutcome::Failed
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ready_publication_durability_uncertainty_is_never_reclassified_failed() {
    let root = tempdir().unwrap();
    let workspace = tempdir().unwrap();
    let controller = SessionController::open(root.path()).unwrap();
    let id = controller
        .create_session(settings(workspace.path()))
        .await
        .unwrap()
        .session
        .id;
    let gate = Arc::new(Gate::default());
    let release = gate.arm_scoped();
    *controller.upload_commit_gate.lock().unwrap() = Some(gate.clone());
    let worker = controller.clone();
    let addressed = id.clone();
    let upload = tokio::spawn(async move {
        worker
            .upload_correlated(
                &addressed,
                None,
                "d".repeat(32),
                vec![file("ready.txt", b"synced")],
            )
            .await
    });
    tokio::task::spawn_blocking(move || gate.wait_entered())
        .await
        .unwrap();
    controller
        .catalog
        .lock()
        .await
        .arm_write_fault_after_rename();
    drop(release);
    assert!(
        matches!(
            upload.await.unwrap(),
            Err(SessionError::CatalogCommit {
                error: super::super::CatalogCommitError::CommittedButDurabilityUncertain { .. }
            })
        ),
        "fault consumed at ready publication"
    );
    let registry = controller
        .catalog
        .lock()
        .await
        .upload_registry(&id)
        .unwrap();
    let (batch, allocation) = registry.allocations.iter().next().unwrap();
    assert!(allocation.ready && !allocation.failed);
    assert_eq!(
        controller
            .upload_status(&id, &"d".repeat(32))
            .await
            .unwrap(),
        UploadOutcome::Ready {
            files: registry.receipts(&id, batch).unwrap()
        }
    );
}
