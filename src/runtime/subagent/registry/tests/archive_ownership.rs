//! Real registry prepare/commit + durable catalog/SQLite + decoded archive.
//! The staged child uses the registry's existing real-process/control seam.
use super::*;
use crate::local_runtime::session::SessionCatalog;
use crate::runtime::identity::SessionId;
use crate::session_archive::{SessionArchiveCut, SessionArchiveProducer};
use std::io::Read;
use tokio_util::sync::CancellationToken;

struct ArchivePlane {
    plane: TestPlane,
    root: tempfile::TempDir,
    catalog: SessionCatalog,
    session: SessionId,
}
impl ArchivePlane {
    fn new() -> Self {
        let (root, catalog, _) = crate::local_runtime::session::tests::open_catalog();
        let session = catalog.list_page(None, 0, 1).unwrap().sessions[0]
            .id
            .clone();
        let conversation = catalog.lineage(&session, None).unwrap().0.conversation_id;
        let mut plane = plane(1);
        plane.store = Arc::new(
            crate::durable::SqliteConversationStore::open(
                conversation.clone(),
                &catalog.database_path(&session, &conversation),
            )
            .unwrap(),
        );
        plane.conversation_id = conversation.clone();
        plane.runtime_root = root.path().to_path_buf();
        let mut config = plane.registry.config.clone();
        config.conversation_id = conversation;
        config.mailbox = ConversationInboundMailbox::over_store(plane.store.clone());
        config.spawn.session_id = session.clone();
        config.spawn.product_root =
            crate::runtime::local_storage::ProductRoot::existing(root.path()).unwrap();
        plane.registry = SubagentRegistry::new(config);
        Self {
            plane,
            root,
            catalog,
            session,
        }
    }

    async fn prepare(&self) -> (PreparedSubagent, ScriptedChild) {
        let child = stage_exit0(&self.plane);
        let prepared = self
            .plane
            .registry
            .prepare(
                &start_spec("archive ownership ordering"),
                &CancellationSignal::new(),
            )
            .await
            .unwrap();
        // The scripted staged process stands in for Ready; materialize the
        // durable child store that real child startup creates before Ready.
        // No ownership event is forged: only registry.commit can publish it.
        let path = self
            .catalog
            .database_path(&self.session, &prepared.child_conversation_id);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        crate::durable::SqliteConversationStore::open(
            prepared.child_conversation_id.clone(),
            &path,
        )
        .unwrap()
        .initialize(&[])
        .unwrap();
        assert!(self.plane.registry.all_snapshots().is_empty());
        assert!(!events(&self.plane).iter().any(|event| matches!(
            event,
            crate::events::types::RuntimeEvent::SubagentOwnershipCommitted { .. }
        )));
        (prepared, child)
    }

    async fn finish(&self, accepted: &SubagentAccepted, child: ScriptedChild) {
        child
            .complete(ChildResultStatus::Succeeded, Some("done"))
            .await;
        self.plane
            .registry
            .wait_until_settled(&accepted.subagent_id)
            .await
            .unwrap();
    }
}

async fn assert_archive_membership(cut: SessionArchiveCut, child: &ConversationId, included: bool) {
    let mut stream = cut.stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.recv().await {
        bytes.extend(chunk.unwrap());
    }
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let mut files = BTreeMap::new();
    for index in 0..zip.len() {
        let mut entry = zip.by_index(index).unwrap();
        let mut text = String::new();
        entry.read_to_string(&mut text).unwrap();
        assert!(files.insert(entry.name().to_owned(), text).is_none());
    }
    let manifest: serde_json::Value = serde_json::from_str(&files["manifest.json"]).unwrap();
    let conversations = manifest["conversations"].as_array().unwrap();
    let count = conversations
        .iter()
        .filter(|c| c["conversation_id"] == child.as_str())
        .count();
    assert_eq!(count, usize::from(included));
    assert_eq!(
        files.contains_key(&format!("sessions/{child}/journal.jsonl")),
        included
    );
    let mut child_claims = 0;
    for parent in conversations {
        let id = parent["conversation_id"].as_str().unwrap();
        let frontier = parent["frontiers"]["journal"].as_i64().unwrap();
        for line in files[&format!("sessions/{id}/journal.jsonl")].lines() {
            let envelope: serde_json::Value = serde_json::from_str(line).unwrap();
            assert!(envelope["sequence"].as_i64().unwrap() <= frontier);
            if envelope["event"]["type"] == "subagent_ownership_committed" {
                let claimed = &envelope["event"]["child_conversation_id"];
                // Directly prohibit the reviewed mixed state for EVERY parent.
                assert_eq!(
                    conversations
                        .iter()
                        .filter(|c| &c["conversation_id"] == claimed)
                        .count(),
                    1,
                    "an included ownership claim requires exactly one included child"
                );
                assert!(conversations.iter().any(|c| &c["conversation_id"] == claimed && c["parent_conversation"] == id));
                if claimed == child.as_str() {
                    child_claims += 1;
                }
            }
        }
    }
    assert_eq!(child_claims, usize::from(included));
}

async fn assert_waiting<F: std::future::Future>(future: std::pin::Pin<&mut F>) {
    let mut future = future;
    std::future::poll_fn(|cx| {
        assert!(
            future.as_mut().poll(cx).is_pending(),
            "ownership commit must wait behind the snapshot"
        );
        std::task::Poll::Ready(())
    })
    .await;
}

#[tokio::test]
async fn archive_wins_before_live_subagent_ownership_commit() {
    let fixture = ArchivePlane::new();
    let (prepared, child) = fixture.prepare().await;
    let child_id = prepared.child_conversation_id.clone();
    let (frozen, reached) = tokio::sync::oneshot::channel();
    let (release, proceed) = std::sync::mpsc::channel();
    let root = fixture.root.path().to_path_buf();
    let session = fixture.session.clone();
    let capture = tokio::task::spawn_blocking(move || {
        SessionArchiveProducer::prepare_inner(&root, &session, &CancellationToken::new(), || {
            frozen.send(()).unwrap();
            proceed.recv().unwrap();
        })
        .unwrap()
    });
    reached.await.unwrap(); // Shared ownership enumeration finished; no cut yet.
    let cancellation = CancellationSignal::new();
    let mut committing = Box::pin(fixture.plane.registry.commit(prepared, &cancellation));
    assert_waiting(committing.as_mut()).await;
    assert!(fixture.plane.registry.all_snapshots().is_empty()); // No registry lock held while waiting.
    assert!(!events(&fixture.plane).iter().any(|event| matches!(
        event,
        crate::events::types::RuntimeEvent::SubagentOwnershipCommitted { .. }
    )));
    release.send(()).unwrap();
    let cut = capture.await.unwrap();
    // Holding an unconsumed archive cannot delay runtime ownership. The cut has
    // released product/SQLite barriers before this commit is allowed to finish.
    let SubagentStartOutcome::Accepted(accepted) = committing.await.unwrap() else {
        panic!("export must not reject execution")
    };
    assert!(events(&fixture.plane).iter().any(|event| matches!(event,
        crate::events::types::RuntimeEvent::SubagentOwnershipCommitted { child_conversation_id, .. } if child_conversation_id == &child_id)));
    assert_archive_membership(cut, &child_id, false).await;
    fixture.finish(&accepted, child).await;
}

#[tokio::test]
async fn live_subagent_ownership_commit_wins_before_archive() {
    let fixture = ArchivePlane::new();
    let (prepared, child) = fixture.prepare().await;
    let child_id = prepared.child_conversation_id.clone();
    let SubagentStartOutcome::Accepted(accepted) = fixture
        .plane
        .registry
        .commit(prepared, &CancellationSignal::new())
        .await
        .unwrap()
    else {
        panic!("accepted")
    };
    let cut = SessionArchiveProducer::prepare(
        fixture.root.path(),
        &fixture.session,
        &CancellationToken::new(),
    )
    .unwrap();
    assert_archive_membership(cut, &child_id, true).await;
    fixture.finish(&accepted, child).await;
}

#[tokio::test]
async fn cancelled_subagent_waiting_for_ownership_admission_rolls_back_without_publication() {
    let fixture = ArchivePlane::new();
    let (prepared, _child) = fixture.prepare().await;
    let freeze = fixture
        .plane
        .registry
        .config
        .spawn
        .product_root
        .freeze_ownership()
        .unwrap();
    let cancellation = CancellationSignal::new();
    let mut committing = Box::pin(fixture.plane.registry.commit(prepared, &cancellation));
    assert_waiting(committing.as_mut()).await;
    cancellation.cancel();
    drop(freeze);
    assert!(matches!(
        committing.await.unwrap(),
        SubagentStartOutcome::RolledBack
    ));
    assert!(fixture.plane.registry.all_snapshots().is_empty());
    assert!(!events(&fixture.plane).iter().any(|event| matches!(
        event,
        crate::events::types::RuntimeEvent::SubagentOwnershipCommitted { .. }
    )));
}

#[tokio::test]
async fn ownership_admission_rechecks_capacity_drain_and_durability() {
    for reason in ["capacity", "drain", "durability"] {
        let fixture = ArchivePlane::new();
        let lifecycle = crate::runtime::types::ConversationLifecycle::new();
        fixture
            .plane
            .registry
            .config
            .mailbox
            .bind_inactive(&lifecycle);
        assert!(lifecycle.activate());
        let durability = Arc::new(DurabilityGate::new());
        fixture
            .plane
            .registry
            .install_durability_gate(durability.clone());
        let (prepared, _child) = fixture.prepare().await;
        let product = &fixture.plane.registry.config.spawn.product_root;
        let freeze = product.freeze_ownership().unwrap();
        let cancellation = CancellationSignal::new();
        let mut committing = Box::pin(fixture.plane.registry.commit(prepared, &cancellation));
        assert_waiting(committing.as_mut()).await;
        // Each operation takes the real competing runtime mutex. None can be
        // held by an ownership admission that is waiting behind inspection.
        match reason {
            "capacity" => fixture.plane.registry.state.lock().unwrap().max_active = 0,
            "drain" => assert!(lifecycle.begin_drain()),
            "durability" => {
                durability.commit_failure(
                    crate::runtime::types::DurableOperation::AdoptPendingBatch,
                    "failed before admission".into(),
                );
            }
            _ => unreachable!(),
        }
        drop(freeze);
        let failure = committing.await.unwrap_err();
        assert!(matches!(
            (reason, failure),
            ("capacity", SubagentStartError::CapacityExceeded { .. })
                | ("drain", SubagentStartError::ConversationInactive)
                | ("durability", SubagentStartError::DurabilityFailed { .. })
        ));
        assert!(fixture.plane.registry.all_snapshots().is_empty());
        assert!(!events(&fixture.plane).iter().any(|event| matches!(
            event,
            crate::events::types::RuntimeEvent::SubagentOwnershipCommitted { .. }
        )));
        let _released = product.freeze_ownership().unwrap();
    }
}

/// Unlike `stage_exit0`, this seam cannot bypass the production allocator.
/// It substitutes a controlled process only after identity/incarnation creation.
fn production_allocation_peer(
    fixture: &ArchivePlane,
) -> (
    tokio::sync::oneshot::Receiver<()>,
    tokio::sync::oneshot::Receiver<ScriptedChild>,
) {
    let (entered, reached) = tokio::sync::oneshot::channel();
    let (created, child) = tokio::sync::oneshot::channel();
    let product = fixture.plane.registry.config.spawn.product_root.clone();
    fixture
        .plane
        .registry
        .state
        .lock()
        .unwrap()
        .allocation_test_hook = Some(AllocationTestHook {
        entered,
        stage: Box::new(move |runtime_root, workspace| {
            // The production allocator, not this peer, created both directories.
            // No ownership admission survives into process staging/Ready.
            let snapshot = product.freeze_ownership().unwrap();
            drop(snapshot);
            let (control, peer) = tokio::net::UnixStream::pair().unwrap();
            let (observation, _observation_peer) = tokio::net::UnixStream::pair().unwrap();
            let process = tokio::process::Command::new("sh")
                .args(["-c", "true"])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .process_group(0)
                .spawn()
                .unwrap();
            let pid = process.id().unwrap();
            assert!(created.send(ScriptedChild { peer, pid }).is_ok());
            StagedChild::for_allocated_test(process, control, observation, runtime_root, workspace)
        }),
    });
    (reached, child)
}

#[tokio::test]
async fn archive_wins_before_production_child_reservation() {
    let fixture = ArchivePlane::new();
    let (allocation_reached, child) = production_allocation_peer(&fixture);
    assert!(
        fixture
            .plane
            .registry
            .state
            .lock()
            .unwrap()
            .staged_overrides
            .is_empty()
    );
    let (frozen, reached) = tokio::sync::oneshot::channel();
    let (release, proceed) = std::sync::mpsc::channel();
    let root = fixture.root.path().to_path_buf();
    let session = fixture.session.clone();
    let capture = tokio::task::spawn_blocking(move || {
        SessionArchiveProducer::prepare_inner(&root, &session, &CancellationToken::new(), || {
            frozen.send(()).unwrap();
            proceed.recv().unwrap();
        })
        .unwrap()
    });
    reached.await.unwrap();
    let spec = start_spec("production reservation behind archive");
    let cancellation = CancellationSignal::new();
    let mut preparing = Box::pin(fixture.plane.registry.prepare(&spec, &cancellation));
    // Poll reaches the real allocator's contested admission, not a staged override.
    assert_waiting(preparing.as_mut()).await;
    allocation_reached.await.unwrap();
    assert_waiting(preparing.as_mut()).await;
    assert!(fixture.plane.registry.all_snapshots().is_empty());
    release.send(()).unwrap();
    let cut = capture.await.unwrap();
    let prepared = preparing
        .await
        .expect("inspection cannot fail runtime reservation");
    let child_id = prepared.child_conversation_id.clone();
    let path = fixture.catalog.database_path(&fixture.session, &child_id);
    assert!(path.parent().unwrap().is_dir());
    assert!(
        !path.exists(),
        "reservation does not fabricate child history"
    );
    assert!(!events(&fixture.plane).iter().any(|event| matches!(
        event,
        crate::events::types::RuntimeEvent::SubagentOwnershipCommitted { .. }
    )));
    assert_archive_membership(cut, &child_id, false).await;
    let SubagentStartOutcome::Accepted(accepted) = fixture
        .plane
        .registry
        .commit(prepared, &cancellation)
        .await
        .unwrap()
    else {
        panic!("accepted")
    };
    assert!(events(&fixture.plane).iter().any(|event| matches!(event,
        crate::events::types::RuntimeEvent::SubagentOwnershipCommitted { child_conversation_id, .. }
        if child_conversation_id == &child_id)));
    fixture.finish(&accepted, child.await.unwrap()).await;
}

#[tokio::test]
async fn cancelled_production_reservation_wait_creates_no_child_allocation() {
    let fixture = ArchivePlane::new();
    let (reached, child) = production_allocation_peer(&fixture);
    let product = &fixture.plane.registry.config.spawn.product_root;
    let conversations = product
        .root()
        .join("sessions")
        .join(fixture.session.as_str())
        .join("conversations");
    let before = std::fs::read_dir(&conversations).unwrap().count();
    let snapshot = product.freeze_ownership().unwrap();
    let spec = start_spec("cancel pending allocation");
    let cancellation = CancellationSignal::new();
    let mut preparing = Box::pin(fixture.plane.registry.prepare(&spec, &cancellation));
    assert_waiting(preparing.as_mut()).await;
    reached.await.unwrap();
    cancellation.cancel();
    drop(snapshot);
    assert!(matches!(
        preparing.await,
        Err(SubagentStartError::Cancelled)
    ));
    assert!(child.await.is_err(), "no process was staged");
    assert_eq!(std::fs::read_dir(conversations).unwrap().count(), before);
    assert!(fixture.plane.registry.all_snapshots().is_empty());
    assert!(!events(&fixture.plane).iter().any(|event| matches!(
        event,
        crate::events::types::RuntimeEvent::SubagentOwnershipCommitted { .. }
    )));
    let _released = product.freeze_ownership().unwrap();
}
